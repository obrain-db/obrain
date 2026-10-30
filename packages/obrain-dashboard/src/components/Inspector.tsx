// Inspector — panneau de configuration escamotable (droit). N'est PAS un
// panneau figé : cest un volet « verre » qui survole le canvas et se replie
// derrière une poignée. Il regroupe : Cible, Échelle & volume, Overlay,
// Strates (couches obrain), et Requête + projection multi-axes.
import { useEffect, useRef } from "react";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import {
  activeStoreAtom, centersAtom, hopsAtom, overlayAtom, limitAtom, layersAtom,
  organicModeAtom, faceAtom, lastGraphAtom, suggestionsAtom,
  queryTextAtom, queryLangAtom, queryResultAtom, queryBusyAtom, projectionVisibleAtom,
  projectionRefusAtom,
} from "../app/store";
import CenterSearch from "./CenterSearch";
import { ChoiceChips } from "./Chips";
import Projection from "./Projection";
import { runQuery } from "../lib/api";
import { refusProjection } from "../lib/projection";

const short = (id: string) => id.split(".").slice(-1)[0];
const SAMPLES: [string, string][] = [
  ["Types de nœuds", "MATCH (n) RETURN labels(n)[0] AS type, count(n) AS nbr ORDER BY nbr DESC LIMIT 8"],
  ["Types de liens", "MATCH ()-[r]->() RETURN type(r) AS t, count(r) AS n ORDER BY n DESC LIMIT 5"],
  ["Callees de e1000_probe", "MATCH (a)-[:CALLS]->(b) WHERE a.qualified_name ENDS WITH '.e1000_probe' RETURN b.qualified_name LIMIT 10"],
  ["Chemins en 1-2 sauts", "MATCH p = (a)-[:CALLS*1..2]->(b) WHERE a.qualified_name CONTAINS 'e1000' RETURN length(p) AS l, count(p) AS c LIMIT 5"],
];

export default function Inspector({ onClose }: { onClose: () => void }) {
  const store = useAtomValue(activeStoreAtom);
  const [centers, setCenters] = useAtom(centersAtom);
  const [hops, setHops] = useAtom(hopsAtom);
  const [overlay, setOverlay] = useAtom(overlayAtom);
  const [limit, setLimit] = useAtom(limitAtom);
  const [layers, setLayers] = useAtom(layersAtom);
  const [organic, setOrganic] = useAtom(organicModeAtom);
  // La face se LIT (dérivée) et s'ÉCRIT par l'unique autorité : l'inspecteur ne
  // peut donc plus afficher « Monde » pendant que la scène montre le graphe.
  const [face, setFace] = useAtom(faceAtom);
  const monde = face === "monde";
  const [sug, setSug] = useAtom(suggestionsAtom);
  const last = useAtomValue(lastGraphAtom);
  const [text, setText] = useAtom(queryTextAtom);
  const [lang, setLang] = useAtom(queryLangAtom);
  const [res, setRes] = useAtom(queryResultAtom);
  const [busy, setBusy] = useAtom(queryBusyAtom);
  const runCtl = useRef<AbortController | null>(null);
  useEffect(() => () => runCtl.current?.abort(), []);
  const setProjVisible = useSetAtom(projectionVisibleAtom);
  const setRefus = useSetAtom(projectionRefusAtom);
  const projVisible = useAtomValue(projectionVisibleAtom);

  const setL = (k: string) => (v: boolean) => setLayers({ ...layers, [k]: v });
  const layerAvail = {
    contour: (last?.edges || []).some((e: any) => e.ricci != null && e.ricci < 0),
    synapse: (last?.edges || []).some((e: any) => (e.weight ?? 0) > 0 || (e.pheromone ?? 0) > 0),
  };
  /**
   * LANCEMENT D'UNE PROJECTION — avec le GARDE-FOU DE VOLUME.
   *
   * DÉFAUT MESURÉ : la projection se lançait sur N'IMPORTE QUELLE réponse. Le
   * serveur ne borne pas `/api/query` (il sérialise `r.rows` tel quel) et le
   * corpus servi fait 413 985 nœuds ; côté client, 50 000 lignes coûtaient déjà
   * ~25 ms de construction pour 64 barres affichées, et l'ancien `Math.min(...)`
   * levait `RangeError` vers 125 000 lignes — dans le rendu, donc en cassant tout
   * l'arbre React (`Inspector` n'est enveloppé d'aucun `CanvasBoundary`).
   *
   * Au-delà de `SEUIL_PROJECTION_ROWS` (20 000), la projection ne se lance donc
   * pas toute seule : `projectionRefusAtom` le DIT (nombre de lignes) et le
   * panneau propose « ajouter LIMIT 20000 » ou « projeter quand même ».
   */
  const run = async (override?: string) => {
    const q = (override ?? text).trim();
    if (!store || !q) return;
    runCtl.current?.abort();
    const ctl = new AbortController();
    runCtl.current = ctl;
    setBusy(true); setProjVisible(true);
    try {
      const r = await runQuery(store, q, lang, ctl.signal);
      setRes(r);
      setRefus(refusProjection((r?.rows || []).length));
    } catch (e: any) {
      if (e?.name !== "AbortError") {
        setRes({ columns: [], rows: [], error: String(e.message || e) });
        setRefus(null);
      }
    } finally { setBusy(false); }
  };

  const layersList = [
    { k: "edges", name: "Appels (CALLS)", d: "Qui appelle qui", c: "#4aa8ff", on: layers.edges, dis: false },
    { k: "tri", name: "Faces K3", d: "Ternaires d'appels", c: "#21d4b4", on: layers.tri, dis: false },
    { k: "k4", name: "Hyperfaces K4", d: "Cliques de 4", c: "#ffd166", on: layers.k4, dis: false },
    { k: "hyper", name: "Hyperarêtes", d: "Étoiles n-aire", c: "#ff7a5c", on: layers.hyper, dis: false },
    { k: "contour", name: "∂ Contour", d: layerAvail.contour ? "Frontières κ<0" : "pas de ricci", c: "#ff6b9d", on: layers.contour, dis: !layerAvail.contour },
    { k: "spectral", name: "Communautés", d: "Couleur par groupe", c: "#b07aa1", on: layers.spectral, dis: false },
    { k: "synapse", name: "Synapses", d: layerAvail.synapse ? "Poids hebbiens" : "pas de poids", c: "#ffe066", on: layers.synapse, dis: !layerAvail.synapse },
  ];

  return (
    <div className="inspector">
      <div className="insp-head">
        <span>Réglages</span>
        {monde && <span className="insp-badge">Monde</span>}
        <button className="ctl-x" aria-label="Fermer" title="Fermer (Échap)" onClick={onClose}>×</button>
      </div>

      <div className="insp-scroll">
        <div className="sec">
          <div className="sec-title">Échelle &amp; volume</div>
          <ChoiceChips options={[
            { value: 1, label: "macro · 1" }, { value: 2, label: "meso · 2" }, { value: 0, label: "complet ∞" }]}
            value={hops} onChange={(v) => { setHops(v); }} />
          <label className="budget">
            <span>nœuds max</span>
            <input type="range" min={40} max={20000} step={20} value={limit}
              onChange={(e) => setLimit(Number(e.target.value))} />
            <b>{limit}</b>
          </label>
        </div>

        <div className="sec">
          <div className="sec-title">Périmètre</div>
          <div className="sec-note">
            {hops === 0 ? "Closure complète depuis le centre (bornée, marquée TRONQUÉ au besoin)."
              : `Voisinage à ${hops} saut${hops > 1 ? "s" : ""} du centre.`}
            <button className="textbtn" onClick={() => { setFace(monde ? "graphe" : "monde"); }}>—&nbsp;{monde ? "quitter le monde" : "store entier"}</button>
          </div>
        </div>

        <div className="sec">
          <div className="sec-title">Overlay</div>
          <ChoiceChips options={[{ value: "", label: "aucun" }, { value: "ricci", label: "ricci" },
            { value: "roles", label: "rôles" }, { value: "energy", label: "énergie" },
            { value: "tier", label: "tiers mémoire" }]}
            value={overlay} onChange={setOverlay} />
        </div>

        <div className="sec">
          <div className="sec-title">Cible</div>
          <CenterSearch />
          {sug.length > 0 && (
            <div className="sug-ambigu">
              <b>⚠ suffixe ambigu</b>
              <div className="chips">
                {sug.map(qn => <button key={qn} className="chip" title={qn}
                  onClick={() => { setSug([]); setCenters([qn]); }}>{short(qn)}</button>)}
                <button className="chip" onClick={() => setSug([])}>✕</button>
              </div>
            </div>
          )}
        </div>

        <div className="sec">
          <div className="sec-title">Strates</div>
          <div className="layerlist">
            {layersList.map(r => (
              <button key={r.k} disabled={r.dis} aria-pressed={r.on}
                className={"lrow" + (r.on ? " on" : "") + (r.dis ? " dis" : "")}
                onClick={() => !r.dis && setL(r.k)(!r.on)}>
                <span className="dot" style={{ background: r.c }} />
                <span className="lname">{r.name}<i>{r.d}</i></span>
                <span className="lswitch">{r.on ? "●" : r.dis ? "✕" : "○"}</span>
              </button>))}
            <button className={"lrow" + (organic ? " on" : "")} onClick={() => setOrganic(o => !o)}>
              <span className="dot" style={{ background: "#7fd1ff" }} />
              <span className="lname">Organique<i>Halos &amp; synapses vives</i></span>
              <span className="lswitch">{organic ? "●" : "○"}</span>
            </button>
          </div>
          <div className="legend-line">
            CALLS <i style={{ background: "#4aa8ff" }} /> K3 <i style={{ background: "#21d4b4" }} />
            K4 <i style={{ background: "#ffd166" }} /> ∂ <i style={{ background: "#ff6b9d" }} />
            syn <i style={{ background: "#ffe066" }} />
          </div>
        </div>

        <div className="sec">
          <div className="sec-title">Requête au moteur</div>
          <div className="q-samples">
            {SAMPLES.map(([l, q]) => <button key={l} className="chip" onClick={() => setText(q)}>{l}</button>)}
          </div>
          <textarea className="q-editor" rows={4} value={text} spellCheck={false}
            onChange={e => setText(e.target.value)} />
          <div className="q-bar">
            <label className="q-lang">Langage
              <select value={lang} onChange={e => setLang(e.target.value)}>
                <option value="gql">GQL</option><option value="cypher">Cypher</option>
              </select>
            </label>
            <button className="btn btn-primary" onClick={() => run()} disabled={busy || !store}>
              {busy ? "…" : "▶ Projeter"}
            </button>
          </div>
          {/* LA PROJECTION EST TOUJOURS ATTEIGNABLE ICI. Défaut mesuré : après un
              clic sur la croix du panneau (`Projection.tsx`), `projectionVisibleAtom`
              restait faux et AUCUN contrôle ne le relisait — la seule façon de
              revoir la projection était de relancer une requête. Elle se rouvre
              donc par ce bouton (et par celui d'`App`, qui rouvre l'inspecteur
              lui-même quand il est fermé). */}
          {!projVisible && (
            <button className="textbtn proj-rouvrir" onClick={() => setProjVisible(true)}>
              ⌗ rouvrir la projection
            </button>
          )}
          <Projection onRun={run} />
        </div>
      </div>
    </div>
  );
}
