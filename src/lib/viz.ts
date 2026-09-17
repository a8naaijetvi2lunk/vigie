// Helpers PURS de data-viz pour le mode étendu :
// géométrie de la sparkline 24h et grille de la heatmap 30j.
// Aucune dépendance Tauri/React — testables et réutilisables tels quels.

/** Échantillon brut retourné par la commande IPC `get_history`. */
export interface Sample {
  ts: number; // epoch secondes
  pct: number;
}

/** Point d'agrégat journalier retourné par la commande IPC `get_heatmap`. */
export interface DailyPoint {
  day: string; // "YYYY-MM-DD" (jour local)
  pct: number;
}

function clampPct(pct: number): number {
  return Math.min(100, Math.max(0, pct));
}

function round1(n: number): number {
  return Math.round(n * 10) / 10;
}

export interface SparklineGeometry {
  /** Attribut `points` prêt pour un `<polyline>` SVG ("x,y x,y …"). */
  points: string;
  /** Coordonnées du dernier échantillon (point accentué --spark-dot). */
  last: { x: number; y: number } | null;
}

/**
 * Géométrie de la sparkline : mappe les échantillons (ts croissants) sur la
 * largeur `width`, pct 0 % → `baselineY` et 100 % → `padTop` (axe y SVG inversé).
 * Le temps est étalé entre le premier et le dernier échantillon (trait pleine
 * largeur, style "enregistreur"). Un échantillon unique se place au bord droit.
 */
export function sparklineGeometry(
  samples: Sample[],
  width: number,
  padTop: number,
  baselineY: number,
): SparklineGeometry {
  if (samples.length === 0) {
    return { points: "", last: null };
  }
  const minTs = samples[0].ts;
  const span = Math.max(1, samples[samples.length - 1].ts - minTs);
  const coords = samples.map((s) => {
    const x = samples.length === 1 ? width : ((s.ts - minTs) / span) * width;
    const y = baselineY - (clampPct(s.pct) / 100) * (baselineY - padTop);
    return { x: round1(x), y: round1(y) };
  });
  return {
    points: coords.map((c) => `${c.x},${c.y}`).join(" "),
    last: coords[coords.length - 1],
  };
}

export type HeatBucket = 0 | 1 | 2 | 3 | 4;

/**
 * Mappe le pct max d'un jour sur la rampe --heat-0..4.
 * Bornes alignées sur les seuils de quota (70 warn / 90 crit) :
 * 0 = vide ou sans donnée, 4 = jour extrême (≥ 90 %).
 */
export function heatBucket(pct: number | null): HeatBucket {
  if (pct === null || pct <= 0) return 0;
  if (pct < 40) return 1;
  if (pct < 70) return 2;
  if (pct < 90) return 3;
  return 4;
}

export interface HeatCell {
  /** Jour "YYYY-MM-DD", ou null pour une cellule de calage (avant le 1er jour). */
  day: string | null;
  /** Pct max du jour, null si sans donnée (ou cellule de calage). */
  pct: number | null;
  bucket: HeatBucket;
  isToday: boolean;
}

/** Jour local "YYYY-MM-DD" d'une Date (sans passer par UTC/toISOString). */
export function isoDayLocal(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}

function parseIsoLocal(iso: string): Date {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(y, m - 1, d);
}

function addDays(base: Date, n: number): Date {
  const copy = new Date(base);
  copy.setDate(copy.getDate() + n);
  return copy;
}

/**
 * Grille heatmap façon GitHub, flux en colonnes de 7 lignes lundi→dimanche :
 * `days` jours consécutifs se terminant à `todayIso`, précédés des cellules de
 * calage (day: null) qui alignent le premier jour sur sa ligne de semaine.
 * Les jours sans donnée dans `points` restent à pct null → bucket 0 (--heat-0).
 */
export function buildHeatmapGrid(
  points: DailyPoint[],
  todayIso: string,
  days = 30,
): HeatCell[] {
  const byDay = new Map(points.map((p) => [p.day, p.pct]));
  const start = addDays(parseIsoLocal(todayIso), -(days - 1));
  const cells: HeatCell[] = [];

  // Calage : index lundi-first (lun=0 … dim=6) du premier jour.
  const offset = (start.getDay() + 6) % 7;
  for (let i = 0; i < offset; i++) {
    cells.push({ day: null, pct: null, bucket: 0, isToday: false });
  }

  for (let i = 0; i < days; i++) {
    const iso = isoDayLocal(addDays(start, i));
    const pct = byDay.get(iso) ?? null;
    cells.push({ day: iso, pct, bucket: heatBucket(pct), isToday: iso === todayIso });
  }
  return cells;
}
