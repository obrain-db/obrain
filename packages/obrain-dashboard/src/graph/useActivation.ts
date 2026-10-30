import { useEffect, useState } from "react";
import { fetchActivation, isAbort, type Activation } from "../lib/api";

/**
 * ACTIVATION EN DIRECT — interroge le journal d'activation à cadence courte.
 *
 * Sondage plutôt que flux poussé : le serveur viz est mono-thread et sans état
 * de session ; un intervalle de 1,2 s suffit à ce que « ça s'allume quand
 * l'engine consulte » soit perçu comme immédiat, sans ajouter de canal.
 * La demi-vie (20 s) est appliquée par le serveur : le client ne recalcule rien.
 */
export function useActivation(ms = 1200): Activation | null {
  const [activation, setActivation] = useState<Activation | null>(null);
  useEffect(() => {
    let live = true;
    const ac = new AbortController();
    const tick = async () => {
      try {
        const a = await fetchActivation(undefined, ac.signal);
        if (live) setActivation(a);
      } catch (e) {
        if (!isAbort(e)) { /* silencieux : l'activation ne doit rien casser */ }
      }
    };
    tick();
    const h = setInterval(tick, ms);
    return () => { live = false; ac.abort(); clearInterval(h); };
  }, [ms]);
  return activation;
}

/** Énergie d'un monde dans [0,1] (0 si rien n'a été consulté). */
export function peakOf(a: Activation | null, store: string): number {
  if (!a) return 0;
  return a.worlds.find((w) => w.store === store)?.peak ?? 0;
}
