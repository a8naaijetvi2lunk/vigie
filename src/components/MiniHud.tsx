import {
  formatHudReset,
  isStale,
  orderedWindows,
  shortModelName,
  stateOf,
  providerStatus,
  type ProviderSnapshot,
} from "../lib/usage";

/**
 * Mode HUD : une seule ligne, façon compteur FPS, pour les petits écrans.
 * Composant de PRÉSENTATION PURE — il ne s'abonne à rien : `CompactCard` reste le
 * seul abonné à `usage-updated` / `config-updated` et lui passe le provider retenu.
 *
 * Sortie du mode : un clic n'importe où sur la ligne (le HUD n'a pas de masthead,
 * donc pas de bouton). La ligne ne porte PAS `data-tauri-drag-region` : l'ancrage
 * est automatique, le déplacement manuel n'a pas lieu d'être.
 */
export default function MiniHud({
  provider,
  onExit,
  activeCount = 0,
}: {
  provider: ProviderSnapshot | undefined;
  onExit: () => void;
  activeCount?: number;
}) {
  const primary = provider ? orderedWindows(provider.windows)[0] : undefined;

  if (!provider || !primary) {
    return (
      <button type="button" className="vigie-window hud-line hud-empty" onClick={onExit} title="Revenir au widget">
        <span className="hud-wait">En attente de données…</span><span aria-hidden="true">↗</span>
      </button>
    );
  }

  const label = provider.model ? shortModelName(provider.model) : provider.prefix;
  const status = providerStatus(provider);

  return (
    <div
      className="vigie-window hud-line"
      data-state={stateOf(primary.usedPercent)}
      data-stale={isStale(provider) ? "true" : "false"}
      role="button"
      tabIndex={0}
      title={`${status ? `${status} · ` : ""}${activeCount} session(s) active(s) · cliquer pour revenir au widget`}
      onClick={onExit}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onExit();
        }
      }}
    >
      <span
        className={`dot ${provider.active ? "dot-active" : "dot-idle"}`}
        title={provider.active ? "agent actif" : "agent idle"}
      />
      {activeCount > 1 && <span className="hud-active-count num" aria-label={`${activeCount} sessions actives`}>{activeCount}</span>}
      <span className="hud-model" key={label}>{label}</span>
      <span className="hud-spacer" />
      <span className="hud-pct num">{status ? "!" : `${Math.round(primary.usedPercent)}%`}</span>
      <span className="hud-reset num">
        {status ? (provider.note === "token_expired" ? "Reconnecter" : "À actualiser") : formatHudReset(primary.kind, primary.resetsAt)}
      </span>
      {/* Jauge de pied : seule information graphique de la ligne. Posée en absolu
          dans le rayon de la fenêtre, elle n'occupe aucune hauteur de flux — le HUD
          reste à 232x34. */}
      <span className="hud-gauge" aria-hidden="true">
        <span
          className="hud-gauge-fill"
          style={{ width: `${status ? 0 : Math.min(100, Math.max(0, primary.usedPercent))}%` }}
        />
      </span>
    </div>
  );
}
