export interface Session {
  id: string;
  providerId: string;
  project: string;
  model: string | null;
  startedAt: number | null;
  lastActiveAt: number;
  active: boolean;
}

export function sinceLabel(epoch: number, now = Date.now() / 1000): string {
  const seconds = Math.max(0, Math.floor(now - epoch));
  if (seconds < 60) return "à l’instant";
  if (seconds < 3600) return `il y a ${Math.floor(seconds / 60)} min`;
  return `il y a ${Math.floor(seconds / 3600)} h`;
}

export function sessionDuration(start: number | null, end: number): string | null {
  if (!start || end < start) return null;
  const minutes = Math.floor((end - start) / 60);
  return minutes >= 60 ? `${Math.floor(minutes / 60)} h ${minutes % 60} min` : `${minutes} min`;
}
