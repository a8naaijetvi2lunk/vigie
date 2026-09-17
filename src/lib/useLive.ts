import { useEffect, useState } from "react";
import { call, subscribe } from "./bridge";

/** S'abonner avant la lecture initiale ferme la fenêtre où un event était perdu. */
export function useLive<T>(command: string, event: string, initial: T) {
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let received = false;
    void (async () => {
      unlisten = await subscribe<T>(event, data => {
        received = true;
        if (!disposed) { setValue(data); setError(null); }
      });
      if (disposed) { unlisten(); return; }
      const data = await call<T>(command);
      if (!disposed && !received) setValue(data);
    })().catch(() => { if (!disposed) setError("Connexion à Vigie indisponible"); });
    return () => { disposed = true; unlisten?.(); };
  }, [command, event]);
  return { value, setValue, error };
}
