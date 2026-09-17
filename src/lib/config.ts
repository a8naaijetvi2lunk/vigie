// Types miroir de la Config Rust (camelCase via serde `rename_all`).
// Source de vérité : src-tauri/src/config.rs.

export interface ProvidersConfig {
  claude: boolean;
  codex: boolean;
}

export interface Config {
  /** Seuils (en %) déclenchant une notification, triés croissant par convention. */
  thresholds: number[];
  /** Notifier aussi la réinitialisation d'une fenêtre de quota. */
  resetNotifications: boolean;
  /** Intervalle nominal (s) entre deux appels à l'API `oauth/usage` de Claude (plancher 300). */
  claudePollIntervalSecs: number;
  alwaysOnTop: boolean;
  autostart: boolean;
  notificationsPaused: boolean;
  /** Opacité de la fenêtre (0.3–1.0). */
  opacity: number;
  /** Fenêtre traversable par les clics souris : réversible via le tray. */
  clickThrough: boolean;
  /** Skin du widget (structure) : "altimetre" (défaut) ou "carnet". */
  skin: string;
  /** Thème de couleurs : "auto" (suit l'OS), "light" ou "dark". */
  theme: string;
  /** Mode HUD minifié (une ligne ancrée dans un coin). Persisté. */
  hud: boolean;
  /** Coin d'ancrage du HUD : "top-right" | "top-left" | "bottom-right" | "bottom-left". */
  hudCorner: string;
  animations: boolean;
  providers: ProvidersConfig;
}

/** Défauts miroir de `Config::default()` côté Rust. */
export const DEFAULT_CONFIG: Config = {
  thresholds: [70, 85, 95],
  resetNotifications: false,
  claudePollIntervalSecs: 300,
  alwaysOnTop: true,
  autostart: false,
  notificationsPaused: false,
  opacity: 1.0,
  clickThrough: false,
  skin: "altimetre",
  theme: "auto",
  hud: false,
  hudCorner: "top-right",
  animations: true,
  providers: { claude: true, codex: true },
};

/** Normalise le skin persisté : tout sauf "carnet" retombe sur "altimetre". */
export function normalizeSkin(skin: string): "altimetre" | "carnet" {
  return skin === "carnet" ? "carnet" : "altimetre";
}

/** Normalise le thème persisté : tout sauf "light"/"dark" retombe sur "auto". */
export function normalizeTheme(theme: string): "auto" | "light" | "dark" {
  return theme === "light" || theme === "dark" ? theme : "auto";
}

export type HudCorner = "top-right" | "top-left" | "bottom-right" | "bottom-left";

/** Normalise le coin persisté : tout ce qui n'est pas un coin connu → "top-right". */
export function normalizeHudCorner(corner: string): HudCorner {
  return corner === "top-left" || corner === "bottom-right" || corner === "bottom-left"
    ? corner
    : "top-right";
}

/** Plancher anti rate-limit de l'intervalle de poll Claude (miroir du plancher Rust). */
export const MIN_POLL_INTERVAL_SECS = 300;

export const OPACITY_MIN = 0.3;
export const OPACITY_MAX = 1.0;

export type ConfigPatch = Omit<Partial<Config>, "providers"> & { providers?: Partial<ProvidersConfig> };

export function mergeConfig(current: Config, patch: ConfigPatch): Config {
  return { ...current, ...patch, providers: { ...current.providers, ...patch.providers } };
}
