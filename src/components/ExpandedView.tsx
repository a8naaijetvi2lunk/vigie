import { useEffect, useState, type ReactNode } from "react";
import { call as invoke } from "../lib/bridge";
import {
  formatResetExact,
  isStale,
  orderedWindows,
  staleLabel,
  stateOf,
  providerStatus,
  type ProviderSnapshot,
} from "../lib/usage";
import {
  buildHeatmapGrid,
  isoDayLocal,
  sparklineGeometry,
  type DailyPoint,
  type Sample,
} from "../lib/viz";

/** Libellés des fenêtres dans la pile de barres. */
const WINDOW_LABELS: Record<string, string> = {
  "5h": "5H",
  weekly: "Hebdo",
  opus: "Opus · Hebdo",
};

/** Kicker court de la section 24h selon la fenêtre tracée. */
const SPARK_KIND_LABELS: Record<string, string> = {
  "5h": "5H",
  weekly: "Hebdo",
  opus: "Opus",
};

const HM_DAY_LABELS = ["lun", "mar", "mer", "jeu", "ven", "sam", "dim"];

/* Géométrie SVG de la sparkline : viewBox 388×78, base y=68. */
const SPARK_W = 388;
const SPARK_H = 78;
const SPARK_PAD_TOP = 8;
const SPARK_BASELINE = 68;
/** Largeur de tracé : 6px de réserve à droite pour que le point final (r=3) ne soit pas rogné. */
const SPARK_DRAW_W = SPARK_W - 6;

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, value));
}

/** % précis : entier tel quel, sinon 1 décimale (ex. "62" / "62.4"). */
function formatPct(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

/** Tête de section éditoriale : "[ 0n ] // Label" + zone droite optionnelle. */
function SectionHead({
  num,
  label,
  right,
}: {
  num: string;
  label: string;
  right?: ReactNode;
}) {
  return (
    <>
      <div className="sec-head">
        <span className="entry-num">
          [ {num} ] <span className="slash">//</span>
        </span>
        <span className="sec-label">{label}</span>
        {right && <span className="sec-right">{right}</span>}
      </div>
      <div className="rule-double" />
    </>
  );
}

/** Sparkline 24h — trait unique style enregistreur, point final accentué. */
function Sparkline({ samples }: { samples: Sample[] }) {
  const geometry = sparklineGeometry(samples, SPARK_DRAW_W, SPARK_PAD_TOP, SPARK_BASELINE);
  return (
    <div className="spark-wrap">
      <svg
        viewBox={`0 0 ${SPARK_W} ${SPARK_H}`}
        role="img"
        aria-label="utilisation sur 24 heures"
      >
        <line
          x1="0"
          y1={SPARK_BASELINE}
          x2={SPARK_W}
          y2={SPARK_BASELINE}
          style={{ stroke: "var(--spark-base)", strokeWidth: 1 }}
        />
        <polyline
          points={geometry.points}
          fill="none"
          style={{ stroke: "var(--spark-stroke)", strokeWidth: 1.5 }}
          strokeLinejoin="round"
          strokeLinecap="round"
        />
        {geometry.last && (
          <circle
            cx={geometry.last.x}
            cy={geometry.last.y}
            r="3"
            style={{ fill: "var(--spark-dot)" }}
          />
        )}
      </svg>
    </div>
  );
}

/** Heatmap 30j façon GitHub : colonnes = semaines, lignes = lun→dim. */
function Heatmap({ points }: { points: DailyPoint[] }) {
  const cells = buildHeatmapGrid(points, isoDayLocal(new Date()));
  return (
    <>
      <div className="heatmap">
        <div className="hm-days">
          {HM_DAY_LABELS.map((d) => (
            <span key={d}>{d}</span>
          ))}
        </div>
        <div className="hm-grid" role="img" aria-label="intensité de consommation sur 30 jours">
          {cells.map((cell, i) =>
            cell.day === null ? (
              <i key={`pad-${i}`} className="hm-pad" aria-hidden="true" />
            ) : (
              <i
                key={cell.day}
                className={`h${cell.bucket}${cell.isToday ? " today" : ""}`}
                title={
                  cell.pct === null
                    ? `${cell.day} : sans donnée`
                    : `${cell.day} : ${formatPct(cell.pct)}%`
                }
              />
            ),
          )}
        </div>
      </div>
      <div className="hm-legend">
        moins<span />
        <i className="h0" />
        <i className="h1" />
        <i className="h2" />
        <i className="h3" />
        <i className="h4" />
        <span />
        plus
      </div>
    </>
  );
}

export default function ExpandedView({
  provider,
  onBack,
}: {
  provider: ProviderSnapshot;
  onBack: () => void;
}) {
  const windows = orderedWindows(provider.windows);
  const primary = windows[0];
  const primaryKind = primary?.kind ?? null;
  const stale = isStale(provider);
  const status = providerStatus(provider);

  // null = chargement en cours, [] = backend répondu sans données.
  const [samples, setSamples] = useState<Sample[] | null>(null);
  const [heatPoints, setHeatPoints] = useState<DailyPoint[] | null>(null);

  // Historique 24h de la fenêtre primaire — rafraîchi à chaque tick de données
  // (provider.dataTs) pour que la sparkline suive les usage-updated.
  useEffect(() => {
    let cancelled = false;
    if (!primaryKind) {
      setSamples([]);
      return;
    }
    invoke<Sample[]>("get_history", { providerId: provider.id, windowKind: primaryKind })
      .then((s) => {
        if (!cancelled) setSamples(s);
      })
      .catch(() => {
        if (!cancelled) setSamples([]);
      });
    return () => {
      cancelled = true;
    };
  }, [provider.id, primaryKind, provider.dataTs]);

  // Agrégat 30j (léger : ≤ 30 lignes) — même cadence de rafraîchissement.
  useEffect(() => {
    let cancelled = false;
    invoke<DailyPoint[]>("get_heatmap", { providerId: provider.id })
      .then((p) => {
        if (!cancelled) setHeatPoints(p);
      })
      .catch(() => {
        if (!cancelled) setHeatPoints([]);
      });
    return () => {
      cancelled = true;
    };
  }, [provider.id, provider.dataTs]);

  return (
    <div className="vigie-window expanded" data-stale={stale ? "true" : "false"}>
      <div className="drag-handle" data-tauri-drag-region aria-hidden="true" />
      <div className="masthead" data-tauri-drag-region>
        <span className="kicker">Vigie</span>
        <button type="button" className="back-btn" onClick={onBack} aria-label="revenir au mode compact">
          ← compact
        </button>
      </div>
      <div className="rule-double" />

      <div className="exp-scroll">
        {/* 01 · Quotas */}
        <SectionHead
          num="01"
          label="Quotas"
          right={
            <>
              <span className="prefix">{provider.prefix}</span>
              <span
                className={`dot ${provider.active ? "dot-active" : "dot-idle"}`}
                title={provider.active ? "agent actif" : "agent idle"}
              />
            </>
          }
        />
        {status && <p className="provider-notice" role="status">{status}{provider.note === "token_expired" ? " · reconnecte Claude Code" : " · dernière mesure conservée"}</p>}
        {primary ? (
          <>
            <div className="exp-display num">
              {Math.round(primary.usedPercent)}
              <span className="unit">%</span>
            </div>
            <div className="exp-reset num">{formatResetExact(primary.kind, primary.resetsAt)}</div>
            <div className="stack-bars">
              {windows.map((w) => (
                <div className="sb-row" key={w.kind}>
                  <span className="sb-label">{WINDOW_LABELS[w.kind] ?? w.kind}</span>
                  <span className="bar">
                    <span
                      className={`bar-fill f-${stateOf(w.usedPercent)}`}
                      style={{ width: `${clampPercent(w.usedPercent)}%` }}
                    />
                    <i className="tick t70" />
                    <i className="tick t90" />
                  </span>
                  <span className="sb-val num">
                    {formatPct(w.usedPercent)}% · {formatResetExact(w.kind, w.resetsAt)}
                  </span>
                </div>
              ))}
            </div>
          </>
        ) : (
          <p className="exp-empty">en attente de données…</p>
        )}

        {/* 02 · 24h */}
        <SectionHead
          num="02"
          label="24h"
          right={
            primaryKind ? (
              <span className="sec-kind">{SPARK_KIND_LABELS[primaryKind] ?? primaryKind}</span>
            ) : undefined
          }
        />
        {samples === null ? (
          <p className="exp-empty">chargement…</p>
        ) : samples.length === 0 ? (
          <p className="exp-empty">pas encore d'historique</p>
        ) : (
          <Sparkline samples={samples} />
        )}

        {/* 03 · 30 jours */}
        <SectionHead num="03" label="30 jours" />
        {heatPoints === null ? (
          <p className="exp-empty">chargement…</p>
        ) : (
          <Heatmap points={heatPoints} />
        )}

        {/* 04 · Activité */}
        <SectionHead num="04" label="Activité" />
        <div className="activity-row">
          <span
            className={`dot ${provider.active ? "dot-active" : "dot-idle"}`}
            aria-hidden="true"
          />
          <span className="activity-label">{provider.active ? "agent actif" : "agent idle"}</span>
          {stale && <span className="activity-stale">{staleLabel(provider.dataTs)}</span>}
        </div>
      </div>
    </div>
  );
}
