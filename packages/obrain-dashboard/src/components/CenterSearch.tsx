import { useState } from "react";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import { activeStoreAtom, centersAtom, requestTokenAtom,
  ancrerAtom,
} from "../app/store";
import { searchFunctions } from "../lib/api";

interface Hit { qn: string; name: string; type?: string; file_path?: string }

const short = (qn: string) => qn.split(".").slice(-1)[0];
const TYPE_DOT: Record<string, string> = { Function: "fn", File: "fl", Struct: "st",
  Enum: "en", Variable: "va", Type: "ty", Method: "me", Class: "cl", Module: "mo" };
const TYPE_CLS: Record<string, string> = { Function: "fn", File: "file" };
/** Contexte court : fichier source serveur, sinon qn. */
function where(h: Hit): string {
  const fp = h.file_path || "";
  const parts = fp.split("/");
  const leaf = parts[parts.length - 1] || "";
  if (leaf) return parts.slice(-2).join("/").replace(leaf, leaf) ? (parts.slice(-2).join("/")) : leaf;
  return "";
}

/** Tri par pertinence : qn exact > nom exact > préfixe > suffixe > contient. */
function rankHits(q: string, hits: Hit[]): Hit[] {
  const ql = q.toLowerCase();
  const score = (h: Hit) => {
    const n = (h.name || "").toLowerCase();
    if (h.qn.toLowerCase() === ql) return 0;
    if (n === ql) return 1;
    if (n.startsWith(ql)) return 2;
    if (n.endsWith(ql)) return 3;
    if (h.qn.toLowerCase().includes(ql)) return 4;
    return 5;
  };
  return [...hits].sort((a, b) => score(a) - score(b) || a.qn.localeCompare(b.qn));
}

export default function CenterSearch() {
  const store = useAtomValue(activeStoreAtom);
  // Lecture du périmètre courant, écriture par l'unique porte (voir `ancrerAtom`).
  const centers = useAtomValue(centersAtom);
  const ancrer = useSetAtom(ancrerAtom);
  const bump = useSetAtom(requestTokenAtom);
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [idx, setIdx] = useState(-1);
  let t: number | undefined;
  let ctrl: AbortController | undefined;

  const onInput = (v: string) => {
    setQ(v); setIdx(-1);
    window.clearTimeout(t);
    if (!store || v.trim().length < 2) return setHits([]);
    ctrl?.abort(); ctrl = new AbortController();
    t = window.setTimeout(async () => {
      try {
        const r = await searchFunctions(store, v.trim(), ctrl!.signal);
        setHits(rankHits(v.trim(), (r.items || []).slice(0, 16)));
      } catch (e) { if ((e as Error).name !== "AbortError") setHits([]); }
    }, 80);
  };

  const add = (qn: string) => {
    if (qn && !centers.includes(qn)) ancrer({ centers: [...centers, qn], label: qn, empiler: false });
    setQ(""); setHits([]); setIdx(-1);
  };
  const del = (qn: string) => ancrer({ centers: centers.filter(c => c !== qn), label: qn, empiler: false });

  return <div className="cs-wrap">
    <input id="obrain-search" value={q} onChange={e => onInput(e.target.value)}
      placeholder="Rechercher une fonction ou un fichier…"
      onKeyDown={e => {
        if (e.key === "Enter") { e.preventDefault(); const h = hits[idx >= 0 ? idx : 0]; add(h ? h.qn : q.trim()); }
        else if (e.key === "ArrowDown") { e.preventDefault(); setIdx(i => Math.min(hits.length - 1, i + 1)); }
        else if (e.key === "ArrowUp") { e.preventDefault(); setIdx(i => Math.max(0, i - 1)); }
        else if (e.key === "Escape") { e.stopPropagation(); setHits([]); setIdx(-1); if (!q) (e.target as HTMLInputElement).blur(); }
      }} />
    {hits.length > 0 && <div className="suggest">
      {hits.map((h, i) => (
        <div key={h.qn} className={i === idx ? "sel" : ""}
          onMouseDown={(ev) => { ev.preventDefault(); add(h.qn); }}
          onMouseEnter={() => setIdx(i)}>
          <span className={"dot " + (TYPE_CLS[h.type || ""] || "fn")} />
          <b>{h.name || short(h.qn)}</b>
          {h.type && <i className="tbadge">{TYPE_DOT[h.type] || h.type}</i>}
          {where(h) && <small> {where(h)}</small>}
        </div>
      ))}
      <div className="sug-hint">↵ = cible · ↑↓ = naviguer</div>
    </div>}
    {q.trim().length >= 2 && hits.length === 0 &&
      <div className="sug-empty">Aucun résultat — essayez un suffixe (ex. tcp_v4_rcv)</div>}
    <div className="chips">
      {centers.map(c => <span key={c} className="chip" title={c}>
        {short(c)}
        <button className="chip-x" onClick={() => del(c)}>✕</button></span>)}
    </div>
  </div>;
}
