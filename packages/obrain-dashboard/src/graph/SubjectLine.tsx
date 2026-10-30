import { useEffect, useState } from "react";
import { useAtom, useSetAtom } from "jotai";
import { subjectAtom, ancrerAtom } from "../app/store";
import { fetchRamifications, isAbort, type Ramifications } from "../lib/api";
import { ACCENT_CSS } from "./grammar";

/**
 * LIGNE DU SUJET — ce que la pièce regarde, affiché par TOUTES les faces —
 * et ses RAMIFICATIONS : tout ce qui part du sujet ou y arrive, groupé par
 * type (le construit), et ce que ses labels portent ailleurs dans le store
 * mais pas encore ici (l'à-construire), représenté au même titre.
 *
 * Chaque ramification réelle est un GESTE : elle ancre le voisinage du sujet
 * (la porte unique `ancrerAtom` — périmètre, fil et rechargement ensemble).
 */
export default function SubjectLine() {
  const [subject] = useAtom(subjectAtom);
  const ancrer = useSetAtom(ancrerAtom);
  const [ram, setRam] = useState<Ramifications | null>(null);
  const [ouvert, setOuvert] = useState(true);

  useEffect(() => {
    setRam(null);
    if (!subject.qn || !subject.store) return;
    const ctl = new AbortController();
    fetchRamifications(subject.store, [subject.qn], ctl.signal)
      .then(setRam)
      .catch((e) => { if (!isAbort(e)) setRam(null); });
    return () => ctl.abort();
  }, [subject.qn, subject.store]);

  if (!subject.label) {
    return (
      <div className="q-intro" style={{ marginTop: 4, opacity: 0.6 }}>
        <b>Sujet</b> : aucun — la pièce n'est pas encore visée.
      </div>
    );
  }
  const chip = (texte: string, titre: string, cliquable: boolean, ghost: boolean, cle: string) => (
    <button
      key={cle}
      title={titre}
      onClick={cliquable ? () => ancrer({ centers: [subject.qn], hops: 1, label: texte }) : undefined}
      style={{
        font: "inherit", fontSize: 11, lineHeight: "18px", padding: "1px 8px",
        borderRadius: 9, cursor: cliquable ? "pointer" : "default",
        border: ghost ? "1px dashed rgba(140,150,170,.55)" : "1px solid rgba(140,150,170,.35)",
        background: ghost ? "transparent" : "rgba(140,150,170,.12)",
        color: ghost ? "rgba(160,170,190,.9)" : "inherit", opacity: ghost ? 0.75 : 1,
      }}
    >{texte}</button>
  );
  const rangee = (items: JSX.Element[], legende: string) =>
    items.length ? (
      <div style={{ display: "flex", flexWrap: "wrap", gap: 4, alignItems: "center", marginTop: 3 }}>
        <span style={{ fontSize: 10, opacity: 0.55, minWidth: 62 }}>{legende}</span>
        {items}
      </div>
    ) : null;
  return (
    <div className="q-intro" style={{ marginTop: 4 }}>
      <div>
        <b>Sujet</b> : <span style={{ color: ACCENT_CSS }}>{subject.label}</span>
        {subject.via ? <span style={{ opacity: 0.6 }}> — posé par {subject.via}</span> : null}
        {ram ? (
          <button onClick={() => setOuvert(!ouvert)} title="Ramifications"
            style={{ font: "inherit", fontSize: 10, marginLeft: 8, padding: "0 6px",
              border: "1px solid rgba(140,150,170,.35)", borderRadius: 8,
              background: "transparent", color: "inherit", cursor: "pointer" }}>
            {ouvert ? "⌄" : "›"} ramifications
          </button>
        ) : null}
      </div>
      {ram && ouvert ? (
        <div>
          {rangee(ram.sortantes.map((r) =>
            chip(`${r.type} ×${r.nb}`, `${r.nb} arête(s) sortante(s) ${r.type} — ancrer le voisinage`, true, false, "s-" + r.type)), "sortantes")}
          {rangee(ram.entrantes.map((r) =>
            chip(`← ${r.type} ×${r.nb}`, `${r.nb} arête(s) entrante(s) ${r.type} — ancrer le voisinage`, true, false, "e-" + r.type)), "entrantes")}
          {rangee(ram.a_construire.sortantes.map((r) =>
            chip(`${r.type}`, `À CONSTRUIRE : les ${ram.labels.join("/")} portent ${r.type} ailleurs (${r.indice_store} dans le store) — pas encore ici`, false, true, "cs-" + r.type)), "à construire")}
          {rangee(ram.a_construire.entrantes.map((r) =>
            chip(`← ${r.type}`, `À CONSTRUIRE (entrant) : ${r.indice_store} dans le store`, false, true, "ce-" + r.type)), "")}
        </div>
      ) : null}
    </div>
  );
}
