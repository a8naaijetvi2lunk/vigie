import { useEffect, useRef, useState } from "react";
import { call } from "../lib/bridge";
import { useLive } from "../lib/useLive";
import { applyAppearance } from "../lib/appearance";
import {
  mergeConfig,
  type ConfigPatch,
  MIN_POLL_INTERVAL_SECS,
  normalizeHudCorner,
  OPACITY_MAX,
  OPACITY_MIN,
  type Config,
} from "../lib/config";
import "./Settings.css";

/** En-tête de section éditorial : numérotation "[ 01 ] //" + titre italique. */
function SectionHead({ num, title }: { num: number; title: string }) {
  return (
    <div className="set-section-head">
      <span className="entry-num">
        [ {String(num).padStart(2, "0")} ] <span className="slash">//</span>
      </span>
      <span className="set-section-title">{title}</span>
    </div>
  );
}

/** Ligne label + toggle charté (pill fine, vert forêt quand actif). */
function ToggleRow({
  label,
  note,
  checked,
  onChange,
}: {
  label: string;
  note?: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <label className="set-row">
      <span className="set-row-label">
        {label}
        {note && <span className="set-row-note">{note}</span>}
      </span>
      <input
        type="checkbox"
        className="set-switch"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
    </label>
  );
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export default function Settings() {
  const [config, setConfig] = useState<Config | null>(null);
  // Drafts texte pour les champs numériques : on laisse taper librement,
  // le clamp/parse ne se fait qu'à l'enregistrement.
  const [thresholdDrafts, setThresholdDrafts] = useState<string[]>(["70", "85", "95"]);
  const [pollDraft, setPollDraft] = useState<string>("300");
  const [saved, setSaved] = useState(false);
  const savedTimer = useRef<number | undefined>(undefined);

  const { value: liveConfig, error: loadError } = useLive<Config | null>("get_config", "config-updated", null);
  const dirty = useRef<ConfigPatch>({});
  const numericDirty = useRef({ thresholds: false, poll: false });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!liveConfig) return;
    setConfig(mergeConfig(liveConfig, dirty.current));
    applyAppearance(liveConfig);
    if (!numericDirty.current.thresholds) {
      const drafts = liveConfig.thresholds.slice(0,3).map(String);
      while (drafts.length < 3) drafts.push("");
      setThresholdDrafts(drafts);
    }
    if (!numericDirty.current.poll) setPollDraft(String(liveConfig.claudePollIntervalSecs));
  }, [liveConfig]);
  useEffect(() => () => { if (savedTimer.current !== undefined) clearTimeout(savedTimer.current); }, []);

  if (!config) return <div className="settings"><p className="set-loading">{loadError ?? "Chargement…"}</p></div>;

  const patch = (partial: ConfigPatch) => {
    dirty.current = { ...dirty.current, ...partial, ...(partial.providers ? { providers: { ...dirty.current.providers, ...partial.providers } } : {}) };
    setConfig(prev => prev ? mergeConfig(prev, partial) : prev);
    setSaved(false);
  };

  async function save() {
    if (!config || saving) return;
    const patch: ConfigPatch = { ...dirty.current };
    if (numericDirty.current.thresholds) {
      const thresholds = thresholdDrafts.map(d => Math.round(Number(d))).filter(n => Number.isFinite(n) && n >= 1 && n <= 100);
      patch.thresholds = [...new Set(thresholds)].sort((a,b) => a-b);
      if (patch.thresholds.length === 0) { setError("Renseigne au moins un seuil entre 1 et 100 %."); return; }
    }
    if (numericDirty.current.poll) {
      const poll = Number(pollDraft);
      if (!Number.isFinite(poll)) { setError("L’intervalle doit être un nombre."); return; }
      patch.claudePollIntervalSecs = Math.min(86400, Math.max(MIN_POLL_INTERVAL_SECS, Math.round(poll)));
    }
    if (patch.opacity !== undefined) patch.opacity = clamp(patch.opacity, OPACITY_MIN, OPACITY_MAX);
    setSaving(true); setError(null);
    try {
      const next = await call<Config>("patch_config", { patch });
      dirty.current = {}; numericDirty.current = { thresholds: false, poll: false };
      setConfig(next);
      const padded = next.thresholds.slice(0,3).map(String);
      while (padded.length < 3) padded.push("");
      setThresholdDrafts(padded); setPollDraft(String(next.claudePollIntervalSecs));
      setSaved(true);
      if (savedTimer.current !== undefined) clearTimeout(savedTimer.current);
      savedTimer.current = window.setTimeout(() => setSaved(false), 2500);
    } catch { setError("Impossible d’enregistrer. Tes modifications sont conservées ici."); }
    finally { setSaving(false); }
  }

  return (
    <div className="settings">
      <div className="set-page" inert={saving}>
        <header className="set-masthead">
          <span className="kicker">Vigie</span>
          <span className="set-masthead-title">Réglages</span>
        </header>
        <div className="rule-double" />

        <section className="set-section">
          <SectionHead num={1} title="Seuils de notification" />
          <div className="set-thresholds">
            {thresholdDrafts.map((draft, i) => (
              <span className="set-threshold" key={i}>
                <input
                  type="number"
                  className="set-input num"
                  min={1}
                  max={100}
                  value={draft}
                  aria-label={`Seuil ${i + 1} en pourcentage`}
                  onChange={(e) => { numericDirty.current.thresholds = true; setThresholdDrafts(prev => prev.map((d,j) => j === i ? e.target.value : d)); }}
                />
                <span className="set-unit">%</span>
              </span>
            ))}
          </div>
          <p className="set-note">Une notification par seuil franchi, par fenêtre de quota.</p>
        </section>

        <section className="set-section">
          <SectionHead num={2} title="Notifications" />
          <ToggleRow
            label="Pause notifications"
            checked={config.notificationsPaused}
            onChange={(v) => patch({ notificationsPaused: v })}
          />
          <ToggleRow
            label="Notifier au reset"
            note="quand une fenêtre de quota se réinitialise"
            checked={config.resetNotifications}
            onChange={(v) => patch({ resetNotifications: v })}
          />
        </section>

        <section className="set-section">
          <SectionHead num={3} title="Fenêtre" />
          <ToggleRow
            label="Toujours au premier plan"
            checked={config.alwaysOnTop}
            onChange={(v) => patch({ alwaysOnTop: v })}
          />
          <div className="set-row">
            <span className="set-row-label">Opacité</span>
            <span className="set-slider-group">
              <input
                type="range"
                aria-label="Opacité"
                className="set-slider"
                min={OPACITY_MIN}
                max={OPACITY_MAX}
                step={0.05}
                value={config.opacity}
                onChange={(e) => patch({ opacity: Number(e.target.value) })}
              />
              <span className="set-slider-value num">{Math.round(config.opacity * 100)}%</span>
            </span>
          </div>
          <ToggleRow
            label="Démarrer avec Windows"
            checked={config.autostart}
            onChange={(v) => patch({ autostart: v })}
          />
          <ToggleRow
            label="Traverser les clics"
            note="la souris passe à travers le widget (à désactiver via le tray)"
            checked={config.clickThrough}
            onChange={(v) => patch({ clickThrough: v })}
          />
        </section>

        <section className="set-section">
          <SectionHead num={4} title="Apparence" />
          <ToggleRow label="Animations douces" note="phare, jauges et transitions d’état" checked={config.animations} onChange={v => patch({ animations: v })} />
          <div className="set-row">
            <span className="set-row-label">
              Skin
              <span className="set-row-note">structure du widget</span>
            </span>
            <select
              className="set-select"
              aria-label="Skin"
              value={config.skin === "carnet" ? "carnet" : "altimetre"}
              onChange={(e) => patch({ skin: e.target.value })}
            >
              <option value="altimetre">Altimètre</option>
              <option value="carnet">Carnet de Veille</option>
            </select>
          </div>
          <div className="set-row">
            <span className="set-row-label">
              Thème
              <span className="set-row-note">couleurs du widget</span>
            </span>
            <select
              className="set-select"
              aria-label="Thème"
              value={
                config.theme === "light" || config.theme === "dark" ? config.theme : "auto"
              }
              onChange={(e) => patch({ theme: e.target.value })}
            >
              <option value="auto">Auto (système)</option>
              <option value="light">Clair</option>
              <option value="dark">Sombre</option>
            </select>
          </div>
          <div className="set-row">
            <span className="set-row-label">
              Coin du HUD
              <span className="set-row-note">ancrage du mode minifié</span>
            </span>
            <select
              className="set-select"
              aria-label="Coin du HUD"
              value={normalizeHudCorner(config.hudCorner)}
              onChange={(e) => patch({ hudCorner: e.target.value })}
            >
              <option value="top-right">Haut droite</option>
              <option value="top-left">Haut gauche</option>
              <option value="bottom-right">Bas droite</option>
              <option value="bottom-left">Bas gauche</option>
            </select>
          </div>
        </section>

        <section className="set-section">
          <SectionHead num={5} title="Claude" />
          <div className="set-row">
            <span className="set-row-label">
              Intervalle de rafraîchissement (s)
              <span className="set-row-note">min 5 min (anti rate-limit)</span>
            </span>
            <input
              type="number"
              className="set-input set-input-wide num"
              aria-label="Intervalle de rafraîchissement en secondes"
              min={MIN_POLL_INTERVAL_SECS}
              step={60}
              value={pollDraft}
              onChange={(e) => { numericDirty.current.poll = true; setPollDraft(e.target.value); }}
            />
          </div>
        </section>

        <section className="set-section">
          <SectionHead num={6} title="Providers" />
          <ToggleRow
            label="Claude"
            note="$ claude — API oauth/usage"
            checked={config.providers.claude}
            onChange={(v) => patch({ providers: { claude: v } })}
          />
          <ToggleRow
            label="Codex"
            note="$ codex — sessions locales"
            checked={config.providers.codex}
            onChange={(v) => patch({ providers: { codex: v } })}
          />
          <p className="set-note">Les sources sont prises en compte sans redémarrer. Le délai anti rate-limit de Claude reste respecté.</p>
        </section>

        {error && <p className="inline-error" role="alert">{error}</p>}
        <footer className="set-footer">
          <span className={`set-saved kicker${saved ? "" : " set-saved-hidden"}`} aria-live="polite">
            Enregistré ✓
          </span>
          <button type="button" className="set-save" onClick={save}>
            {saving ? "Enregistrement…" : "Enregistrer"}
          </button>
        </footer>
      </div>
    </div>
  );
}
