// CommandBar — surface unique posée en haut du canvas : store, recherche de
// fonction (autocomplete), et commande libre (question naturelle / requête
// GQL). Tout le reste vit dans l'inspecteur escamotable : la représentation
// reste maîtresse.
import { useEffect, useRef, useState } from "react";
import { searchFunctions } from "../lib/api";
import type { Anchor } from "../app/store";

interface Hit { qn: string; name: string; type?: string; file_path?: string }

interface P {
  value: string;
  onChange: (v: string) => void;
  onSubmit: (text: string) => void;   // interprétation (naturel / GQL / symbole)
  onAnchor: (qn: string) => void;     // ancrer un qualified_name dans le graphe
  presets: { label: string; path: string }[];
  activeLabel: string;         // label du preset actif (valeur du <select>)
  store: string;               // chemin du store actif (pour l'autocomplete)
  onGoPreset: (label: string) => void;
  anchors: Anchor[];
  onAnchorNav: (idx: number) => void;
}

const short = (qn: string) => qn.split(".").slice(-1)[0];
const where = (h: Hit): string => {
  const leaf = (h.file_path || "").split("/").slice(-2).join("/");
  return leaf || h.qn;
};

export default function CommandBar(p: P) {
  const [hits, setHits] = useState<Hit[]>([]);
  const [idx, setIdx] = useState(-1);
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const t = useRef<number | undefined>(undefined);
  const ctrl = useRef<AbortController | undefined>(undefined);

  useEffect(() => {
    // ⌘K / « / » depuis n'importe où → focus commande
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName || "";
      const typing = tag === "INPUT" || tag === "TEXTAREA";
      const want = e.key === "/" || ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k");
      if (want && !typing) { e.preventDefault(); inputRef.current?.focus(); }
      if (e.key === "Escape" && document.activeElement === inputRef.current) { setOpen(false); inputRef.current?.blur(); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onInput = (v: string) => {
    p.onChange(v);
    setIdx(-1);
    window.clearTimeout(t.current);
    if (v.trim().length < 2) { setHits([]); setBusy(false); return; }
    ctrl.current?.abort(); ctrl.current = new AbortController();
    setBusy(true);
    t.current = window.setTimeout(async () => {
      try {
        const r = await searchFunctions(p.store || "", v.trim(), ctrl.current!.signal);
        setHits((((r as any)?.items) || []).slice(0, 8));
      } catch (e) { if ((e as Error).name !== "AbortError") setHits([]); }
      finally { setBusy(false); }
    }, 90);
  };

  const submit = () => {
    const live = (inputRef.current?.value ?? p.value).trim();
    const h = hits[idx >= 0 ? idx : 0];
    if (h) { setOpen(false); setHits([]); p.onAnchor(h.qn); setIdx(-1); p.onChange(""); return; }
    p.onSubmit(live);
    setOpen(false); setHits([]); setIdx(-1);
    p.onChange("");
  };

  return (
    <div className="cmdbar">
      <div className="cmdbar-inner">
        <span className="cmd-brand" title="Obrain — exploration structurelle">⛁</span>
        <label className="cmd-store">
          <select value={p.activeLabel} onChange={(e) => p.onGoPreset(e.target.value)}>
            {p.presets.map((x) => <option key={x.path} value={x.label}>{x.label}</option>)}
          </select>
        </label>
        <div className="cmd-input-wrap">
          <span className="cmd-prompt">{busy ? "…" : "❯"}</span>
          <input
            ref={inputRef}
            className="cmd-input"
            placeholder="Rechercher une fonction · une question · une requête (⌘K)"
            value={p.value}
            onChange={(e) => onInput(e.target.value)}
            onFocus={() => setOpen(true)}
            onKeyDown={(e) => {
              if (e.key === "Enter") { e.preventDefault(); submit(); }
              else if (e.key === "ArrowDown") { e.preventDefault(); setIdx((i) => Math.min(hits.length - 1, i + 1)); }
              else if (e.key === "ArrowUp") { e.preventDefault(); setIdx((i) => Math.max(0, i - 1)); }
            }}
            spellCheck={false}
          />
          {hits.length > 0 && (
            <div className="cmd-suggest">
              {hits.map((h, i) => (
                <div key={h.qn} className={"cmd-hit" + (i === idx ? " sel" : "")}
                  onMouseDown={(ev) => { ev.preventDefault(); setOpen(false); setHits([]); p.onAnchor(h.qn); p.onChange(""); }}
                  onMouseEnter={() => setIdx(i)}>
                  <span className="hit-dot" />
                  <b>{h.name || short(h.qn)}</b>
                  {h.type && <i className="hit-t">{h.type}</i>}
                  <small>{where(h)}</small>
                </div>
              ))}
              <div className="cmd-hint">↵ = cible · ↑↓ = naviguer · ⌫</div>
            </div>
          )}
        </div>
        <span className="cmd-keys">⌘K</span>
      </div>

      {p.anchors.length > 0 && (
        <div className="cmd-breadcrumbs">
          <span className="cmd-bc-lbl">fil</span>
          {p.anchors.map((a, i) => (
            <button key={i} className={"cmd-bc" + (i === p.anchors.length - 1 ? " on" : "")}
              title={a.label}
              onClick={() => p.onAnchorNav(i)}>{short(a.label)}</button>
          ))}
        </div>
      )}
    </div>
  );
}
