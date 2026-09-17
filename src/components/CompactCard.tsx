import { Fragment, useEffect, useState } from "react";
import { call } from "../lib/bridge";
import { useLive } from "../lib/useLive";
import { applyAppearance } from "../lib/appearance";
import { type Session } from "../lib/sessions";
import Brand from "./Brand";
import { DEFAULT_CONFIG, normalizeSkin, normalizeTheme, type Config } from "../lib/config";
import {
  formatCountdown,
  isStale,
  orderedWindows,
  pickHudProvider,
  staleLabel,
  stateOf,
  type ProviderSnapshot,
  type QuotaWindow,
  type Snapshot,
} from "../lib/usage";
import ExpandedView from "./ExpandedView";
import MiniHud from "./MiniHud";

/** Applique la config d'apparence (opacité, skin, thème) au `:root` de CETTE
 * fenêtre : chaque fenêtre Tauri a son propre document. En thème "auto",
 * l'attribut `data-theme` est retiré pour laisser jouer `prefers-color-scheme`. */
function applyWindowConfig(config: Config): void {
  applyAppearance(config);
  const root = document.documentElement;
  root.style.setProperty("--widget-opacity", String(config.opacity));
  root.setAttribute("data-skin", normalizeSkin(config.skin));
  const theme = normalizeTheme(config.theme);
  if (theme === "auto") {
    root.removeAttribute("data-theme");
  } else {
    root.setAttribute("data-theme", theme);
  }
}

/** Cycle du bouton thème : auto → clair → sombre → auto. */
const THEME_CYCLE: Record<"auto" | "light" | "dark", "auto" | "light" | "dark"> = {
  auto: "light",
  light: "dark",
  dark: "auto",
};

/** Noms français des thèmes pour les labels accessibles. */
const THEME_NAMES: Record<"auto" | "light" | "dark", string> = {
  auto: "auto",
  light: "clair",
  dark: "sombre",
};

/** Libellé du kicker (haut droite de bande) selon la fenêtre primaire. */
const KICKER_LABELS: Record<string, string> = {
  "5h": "5H",
  weekly: "Hebdo",
  opus: "Opus",
};

/** Libellé mono des lignes secondaires. */
const SECONDARY_LABELS: Record<string, string> = {
  "5h": "5h",
  weekly: "Hebdo",
  opus: "Opus hebdo",
};

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, value));
}

/** Fenêtre primaire : gros pourcentage + countdown + barre fine avec encoches 70/90.
 * Le `.kicker-5h` (carnet-only) porte le label de fenêtre à côté de la lettrine —
 * en skin Carnet, le kicker de la ligne méta est masqué (CSS) au profit de celui-ci. */
function PrimaryBlock({ window }: { window: QuotaWindow }) {
  return (
    <>
      <div className="band-display">
        <span className="display-value num">
          {Math.round(window.usedPercent)}
          <span className="unit">%</span>
        </span>
        <span className="kicker-5h carnet-only">
          {SECONDARY_LABELS[window.kind] ?? window.kind}
        </span>
        <span className="countdown num">{formatCountdown(window.kind, window.resetsAt)}</span>
      </div>
      <div className="bar bar-primary">
        <span
          className="bar-fill"
          style={{ width: `${clampPercent(window.usedPercent)}%` }}
        />
        <i className="tick t70" />
        <i className="tick t90" />
      </div>
    </>
  );
}

/** Fenêtre secondaire : label mono + mini-barre + "X% · reset …".
 * `critCaveat` (bande critique, skin Carnet) : l'annotation Caveat « ça chauffe ! »
 * remplace le label de la première rangée (une seule manuscrite par vue). */
function SecondaryRow({ window, critCaveat }: { window: QuotaWindow; critCaveat?: boolean }) {
  return (
    <div className="band-secondary">
      {critCaveat && <span className="caveat-crit carnet-only">ça chauffe&nbsp;!</span>}
      <span className="wk-label">{SECONDARY_LABELS[window.kind] ?? window.kind}</span>
      <span className="bar bar-mini">
        <span
          className={`bar-fill f-${stateOf(window.usedPercent)}`}
          style={{ width: `${clampPercent(window.usedPercent)}%` }}
        />
      </span>
      <span className="wk-meta num">
        {Math.round(window.usedPercent)}% · {formatCountdown(window.kind, window.resetsAt)}
      </span>
    </div>
  );
}

function ProviderBand({
  provider,
  index,
  onSelect,
}: {
  provider: ProviderSnapshot;
  index: number;
  onSelect: () => void;
}) {
  const windows = orderedWindows(provider.windows);
  const primary = windows[0];
  const secondaries = windows.slice(1);
  const bandState = primary ? stateOf(primary.usedPercent) : "ok";
  const stale = isStale(provider);
  const tokenExpired = provider.note === "token_expired";
  const rateLimited = provider.note === "rate_limited";
  const unavailable = provider.note === "unavailable";

  return (
    <div
      className="band band-clickable"
      data-state={bandState}
      data-stale={stale ? "true" : "false"}
      role="button"
      tabIndex={0}
      title="voir le détail"
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
    >
      <div className="band-id">
        <span className="entry-num">
          [ {String(index + 1).padStart(2, "0")} ] <span className="slash">//</span>
        </span>
        <span className="prefix">{provider.prefix}</span>
        <span
          className={`dot ${provider.active ? "dot-active" : "dot-idle"}`}
          title={provider.active ? "agent actif" : "agent idle"}
        />
        <span className="spacer" />
        {unavailable ? <span className="kicker kicker-mute">Indisponible</span> : rateLimited ? (
          <span className="kicker kicker-mute" title="rate-limited par l'API">
            rate-limited
          </span>
        ) : stale ? (
          <span className="kicker kicker-mute">{staleLabel(provider.dataTs)}</span>
        ) : primary ? (
          <span className="kicker">{KICKER_LABELS[primary.kind] ?? primary.kind}</span>
        ) : null}
      </div>

      {tokenExpired ? (
        <div className="band-display">
          <span className="countdown countdown-mute">reconnecte Claude Code</span>
        </div>
      ) : primary ? (
        <>
          <PrimaryBlock window={primary} />
          {secondaries.map((w, i) => (
            <SecondaryRow key={w.kind} window={w} critCaveat={i === 0 && bandState === "crit"} />
          ))}
        </>
      ) : (
        <div className="band-display">
          <span className="countdown countdown-mute">en attente de données…</span>
        </div>
      )}
    </div>
  );
}

export default function CompactCard() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  /** id du provider affiché en mode étendu, ou null en compact. */
  const [expandedId, setExpandedId] = useState<string | null>(null);
  /** Config courante (opacité + skin + thème) — null tant que get_config n'a pas répondu. */
  const [config, setConfig] = useState<Config | null>(null);

  /** Horloge locale (ms), rafraîchie toutes les 30 s. Sans elle, la rotation du HUD
   * entre providers actifs et les countdowns ne bougeraient qu'à l'arrivée d'un event
   * — or Codex peut rester silencieux plusieurs heures. */
  const [tick, setTick] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setTick(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);

  const { value: liveSnapshot } = useLive<Snapshot | null>("get_snapshot", "usage-updated", null);
  const { value: liveConfig } = useLive<Config | null>("get_config", "config-updated", null);
  const { value: sessions } = useLive<Session[]>("get_sessions", "sessions-updated", []);
  const [actionError, setActionError] = useState<string | null>(null);
  useEffect(() => { if (liveSnapshot) setSnapshot(liveSnapshot); }, [liveSnapshot]);
  useEffect(() => {
    if (liveConfig) { setConfig(liveConfig); applyWindowConfig(liveConfig); }
  }, [liveConfig]);
  const patchConfig = async (partial: Partial<Config>) => {
    try {
      const next = await call<Config>("patch_config", { patch: partial });
      setConfig(next); applyWindowConfig(next); setActionError(null);
    } catch { setActionError("Réglage non enregistré"); }
  };

  const skin = normalizeSkin(config?.skin ?? DEFAULT_CONFIG.skin);
  const theme = normalizeTheme(config?.theme ?? DEFAULT_CONFIG.theme);
  const skinLabel =
    skin === "carnet"
      ? "Skin : Carnet de Veille — cliquer pour Altimètre"
      : "Skin : Altimètre — cliquer pour Carnet de Veille";
  const themeLabel = `Thème : ${THEME_NAMES[theme]} — cliquer pour ${THEME_NAMES[THEME_CYCLE[theme]]}`;
  const toggleSkin = () => patchConfig({ skin: skin === "carnet" ? "altimetre" : "carnet" });
  const cycleTheme = () => patchConfig({ theme: THEME_CYCLE[theme] });

  const providers = (snapshot?.providers ?? []).filter(p => !config || (p.id === "claude" ? config.providers.claude : config.providers.codex));
  const activeCount = sessions.filter(s => s.active && (!config || (s.providerId === "claude" ? config.providers.claude : config.providers.codex))).length;
  const expandedProvider =
    expandedId !== null ? providers.find((p) => p.id === expandedId) : undefined;
  const hudEnabled = config?.hud ?? DEFAULT_CONFIG.hud;
  /** Vue effective : le HUD prime sur le mode étendu (les deux sont exclusifs). */
  const view: "compact" | "expanded" | "hud" = hudEnabled
    ? "hud"
    : expandedProvider !== undefined
      ? "expanded"
      : "compact";

  // Taille et position de fenêtre pilotées côté Rust — synchronisées sur la vue
  // effective (couvre aussi le 1er rendu : re-normalise si le plugin window-state
  // a restauré une géométrie d'une session précédente).
  useEffect(() => {
    call("set_view", { view }).catch(() => {
      // Hors contexte Tauri (vite dev navigateur) : bascule CSS seule.
    });
  }, [view, config?.hudCorner]);

  if (view === "hud") {
    return (
      <MiniHud
        provider={pickHudProvider(providers, tick)}
        activeCount={activeCount}
        onExit={() => patchConfig({ hud: false })}
      />
    );
  }

  if (view === "expanded" && expandedProvider) {
    return <ExpandedView provider={expandedProvider} onBack={() => setExpandedId(null)} />;
  }

  return (
    <div className="vigie-window compact">
      <div className="drag-handle" data-tauri-drag-region aria-hidden="true" />
      <div className="masthead" data-tauri-drag-region>
        <Brand active={activeCount > 0} />
        <span className="chrome">
          <button className="ctl ctl-sessions" aria-label={`Voir les sessions : ${activeCount} actives`} title="Sessions actives" onClick={() => call("open_sessions").catch(() => setActionError("Sessions indisponibles"))}><span className="num">{activeCount}</span><span aria-hidden="true">◉</span></button>
          <button
            type="button"
            className="ctl ctl-skin"
            onClick={toggleSkin}
            title={skinLabel}
            aria-label={skinLabel}
          >
            <span className="glyph" aria-hidden="true" />
          </button>
          <button
            type="button"
            className="ctl ctl-theme"
            onClick={cycleTheme}
            title={themeLabel}
            aria-label={themeLabel}
          >
            <span className="glyph" aria-hidden="true">
              ◐
            </span>
          </button>
          <button
            type="button"
            className="ctl ctl-hud"
            onClick={() => patchConfig({ hud: true })}
            title="Mode HUD — une ligne ancrée dans un coin de l'écran"
            aria-label="Passer en mode HUD"
          >
            <span className="glyph" aria-hidden="true" />
          </button>
        </span>
      </div>
      <div className="rule-double" />
      {actionError && <span className="widget-error" role="alert">{actionError}</span>}

      {providers.length === 0 ? (
        <div className="band" data-state="ok">
          <div className="band-display">
            <span className="countdown countdown-mute">en attente de données…</span>
          </div>
        </div>
      ) : (
        providers.map((provider, index) => (
          <Fragment key={provider.id}>
            {index > 0 && <div className="rule-single" />}
            <ProviderBand
              provider={provider}
              index={index}
              onSelect={() => setExpandedId(provider.id)}
            />
          </Fragment>
        ))
      )}
    </div>
  );
}
