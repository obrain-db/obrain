/**
 * ── EXPLORATEUR DE SESSIONS ──────────────────────────────────────────────────
 *
 * Liste les sessions du store runtime (les plus récentes d'abord) et rend une
 * sélection cliquable : choisir une session ouvre sa timeline. Composant PUR
 * côté rendu (aucune mutation) — il reçoit `store`, `selection` et `onChoisir`.
 */
import { useEffect, useState } from "react";
import { fetchSessions, SessionBref } from "../lib/api";

export default function SessionList({
  store,
  selection,
  onChoisir,
}: {
  store?: string;
  selection?: string;
  onChoisir: (qn: string) => void;
}) {
  const [sessions, setSessions] = useState<SessionBref[]>([]);
  const [erreur, setErreur] = useState<string | null>(null);

  useEffect(() => {
    const ctl = new AbortController();
    setErreur(null);
    fetchSessions(store, 200, ctl.signal)
      .then((r) => setSessions(r.sessions || []))
      .catch((e) => {
        if ((e as Error)?.name !== "AbortError") setErreur(String((e as Error)?.message || e));
      });
    return () => ctl.abort();
  }, [store]);

  if (erreur) return <div className="sl-panneau sl-erreur">Sessions indisponibles : {erreur}</div>;

  return (
    <div className="sl-panneau">
      <div className="sl-tete">{sessions.length} session{sessions.length > 1 ? "s" : ""}</div>
      <ol className="sl-liste">
        {sessions.map((s) => (
          <li key={s.qn}>
            <button
              className={"sl-item" + (s.qn === selection ? " on" : "")}
              onClick={() => onChoisir(s.qn)}
              title={s.qn}
            >
              <span className="sl-titre">{titre(s)}</span>
              <span className="sl-meta">{s.source_path || s.createdAt || s.qn}</span>
            </button>
          </li>
        ))}
      </ol>
    </div>
  );
}

/** Un titre qui n'apprend rien ne s'affiche pas comme un nom inventé. */
function titre(s: SessionBref): string {
  const t = (s.title || "").trim();
  if (t) return t.length > 80 ? t.slice(0, 80) + "…" : t;
  return s.qn.replace(/^session-/, "").slice(0, 8) + "…";
}
