import { useEffect, useState } from "react";
import { call } from "../lib/bridge";
import { DEFAULT_CONFIG, type Config } from "../lib/config";
import { type Session, sinceLabel, sessionDuration } from "../lib/sessions";
import { shortModelName, providerStatus, orderedWindows, type Snapshot } from "../lib/usage";
import { useLive } from "../lib/useLive";
import { applyAppearance } from "../lib/appearance";
import Brand from "./Brand";

export default function SessionsView() {
  const { value: sessions, error } = useLive<Session[]>("get_sessions", "sessions-updated", []);
  const { value: config } = useLive<Config>("get_config", "config-updated", DEFAULT_CONFIG);
  const { value: snapshot } = useLive<Snapshot>("get_snapshot", "usage-updated", { providers: [], fetchedAt: 0 });
  const [filter, setFilter] = useState("all");
  const [actionError, setActionError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => { applyAppearance(config); }, [config]);
  useEffect(() => { const id = setInterval(() => setNow(Date.now() / 1000), 15000); return () => clearInterval(id); }, []);
  const enabled = sessions.filter(s => s.providerId === "claude" ? config.providers.claude : config.providers.codex);
  const active = enabled.filter(s => s.active).length;
  const visible = enabled.filter(s => filter === "all" || (filter === "active" ? s.active : s.providerId === filter));
  const pause = async () => {
    try { await call("patch_config", { patch: { notificationsPaused: !config.notificationsPaused } }); setActionError(null); }
    catch { setActionError("La modification n’a pas pu être enregistrée."); }
  };
  return <main className="sessions-page">
    <header className="sessions-header"><Brand active={active > 0} /><span className="live-tag"><i className={`dot ${active ? "dot-active" : "dot-idle"}`} /> Veille locale</span></header>
    <section className="sessions-intro"><span className="kicker">Votre atelier</span><h1>{active > 0 ? <><span className="num">{active}</span> session{active > 1 ? "s" : ""} active{active > 1 ? "s" : ""}</> : "Tout est calme."}</h1><p>Vos projets, un regard suffit.</p></section>
    <div className="sessions-quotas">{snapshot.providers.filter(p => config.providers[p.id as keyof typeof config.providers]).map(p => {
      const primary = orderedWindows(p.windows)[0];
      return <div className="session-quota" key={p.id}><span>{p.id === "claude" ? "Claude" : "Codex"}</span><strong className="num" title={providerStatus(p) ?? undefined}>{providerStatus(p) ? "À actualiser" : primary ? `${Math.round(primary.usedPercent)}%` : "En attente"}</strong></div>;
    })}</div>
    <nav className="session-filters" aria-label="Filtrer les sessions">{[["all", "Toutes"], ["active", "Actives"], ["claude", "Claude"], ["codex", "Codex"]].map(([id,label]) => <button key={id} aria-pressed={filter === id} onClick={() => setFilter(id)}>{label}</button>)}</nav>
    <div className="session-list" aria-label="Sessions récentes">
      {visible.map(s => <article className="session-card" key={s.id} data-active={s.active}>
        <div className="session-card-top"><span className={`session-provider ${s.providerId}`}>{s.providerId === "claude" ? "C" : "X"}</span><div className="session-project"><h2 title={s.project}>{s.project}</h2><span>{s.model ? shortModelName(s.model) : s.providerId === "claude" ? "Claude" : "Codex"}</span></div><span className="session-status" title="Actif = écriture observée depuis moins de 30 secondes"><i className={`dot ${s.active ? "dot-active" : "dot-idle"}`} />{s.active ? "Actif" : "Silencieux"}</span></div>
        <div className="session-meta"><span>{sinceLabel(s.lastActiveAt, now)}</span>{sessionDuration(s.startedAt, s.lastActiveAt) && <span>Session · {sessionDuration(s.startedAt, s.lastActiveAt)}</span>}</div>
      </article>)}
      {visible.length === 0 && <div className="sessions-empty"><img src={new URL("../assets/vigie-mark.svg", import.meta.url).href} alt="" /><h2>{filter === "active" ? "Une pause bien méritée." : "Le phare est prêt."}</h2><p>{filter === "active" ? "Aucune activité récente observée." : "Les sessions Claude et Codex apparaîtront ici à leur prochaine activité."}</p></div>}
    </div>
    <footer className="sessions-footer"><p>Activité observée sur les dernières 24 h.<br />Une session silencieuse peut encore travailler.</p><button className="quiet-button" onClick={pause}>{config.notificationsPaused ? "Reprendre les alertes" : "Mettre les alertes en pause"}</button></footer>
    {(error || actionError) && <p className="inline-error" role="alert">{error || actionError}</p>}
  </main>;
}
