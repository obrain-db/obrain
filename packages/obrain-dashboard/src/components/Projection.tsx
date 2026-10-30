// Projection — rend la réponse moteur en axes multi-dimensionnels organisés
// structurellement (type / module / communauté / énergie / volume). Chaque axe
// est une distribution en barres ; les axes « ancrables » (qualified_name)
// permettent de replonger chaque groupe dans le graphe.
//
// ── CE QUI ÉTAIT CASSÉ ICI (mesuré le 2026-09-13, sonde CDP sur le service 8432)
//
//  1. `if (!visible || !res) return null` : avant la première requête, le panneau
//     était MUET alors que le fichier savait déjà dire « réponse vide ». Il dit
//     maintenant explicitement qu'aucune requête n'a été exécutée.
//  2. Après un clic sur la croix, `projectionVisibleAtom` passait à faux et plus
//     AUCUN contrôle ne le relisait : la projection ne revenait qu'en relançant
//     une requête. Elle se rouvre désormais depuis l'inspecteur et depuis `App`.
//  3. `.slice(0, 16)` : le plafond de 16 était un plafond d'AFFICHAGE appliqué
//     APRÈS que `projectAxes` ait construit tous les groupes (8 000 items
//     mesurés sur 8 000 lignes). Le panneau n'en disait rien : la traîne était
//     invisible ET payée. Il l'annonce maintenant (« N groupes · les K plus gros
//     affichés · M masqués ») et propose de la parcourir.
//  4. Un clic sur un groupe n'Ancrait QUE dans le graphe : sa part dans son axe
//     (rang, pourcentage, effectif) n'était affichée nulle part.
//  5. `bump` (setter de `requestTokenAtom`) et `store` (`activeStoreAtom`) étaient
//     déclarés et JAMAIS utilisés : deux abonnements pour rien, et un lecteur de
//     moins pour des atoms qui changeaient sans que le panneau s'en serve.
import { useEffect, useMemo, useState } from "react";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import { queryResultAtom, queryTextAtom, projectionVisibleAtom,
  projectionRefusAtom, ancrerAtom,
} from "../app/store";
import { projectAxes, axisTotal, axisItemsFull, withLimit, SEUIL_PROJECTION_ROWS,
  TOP_GROUPES, type ProjectAxis, type ProjectItem } from "../lib/projection";

const AXIS_COLOR = ["#4aa8ff", "#21d4b4", "#ffd166", "#b07aa1", "#ff7a5c", "#ff6b9d", "#ffe066"];

// ── LA LISTE DÉFILANTE, FENÊTRÉE ────────────────────────────────────────────
// « Tout voir » peut porter 8 000 à 413 985 groupes : les poser tous dans le DOM
// figerait l'onglet (c'est le défaut qu'on répare). Seules les lignes de la
// fenêtre visible (± 8 de marge) existent donc — le défilement est un simple
// décalage, pas un montage.
const H_ROW = 22;
const H_VIEW = 264;
const OVERSCAN = 8;

function Ligne({ it, couleur, total, rang, onPick, actif }:
{ it: ProjectItem; couleur: string; total: number; rang: number;
  onPick: () => void; actif: boolean }) {
  return (
    <button className={"proj-row" + (actif ? " on" : "")} style={{ height: H_ROW }}
      onClick={onPick} title={it.label}>
      <span className="proj-row-lbl">
        <i className="proj-rang">{rang}</i> {it.label}{" "}
        <i className="proj-share">{total ? Math.round((it.value / total) * 100) : 0}%</i>
      </span>
      <span className="proj-track">
        <span className="proj-fill" style={{
          width: `${total ? Math.min(100, (it.value / total) * 100) : 0}%`,
          background: couleur,
        }} />
      </span>
      <span className="proj-row-val">{it.value}</span>
    </button>
  );
}

function ListeFenetree({ items, total, couleur, couleur2, onPick, sel }:
{ items: ProjectItem[]; total: number; couleur: string; couleur2: string;
  onPick: (it: ProjectItem, rang: number) => void; sel: string | null }) {
  const [top, setTop] = useState(0);
  const n = items.length;
  const debut = Math.max(0, Math.floor(top / H_ROW) - OVERSCAN);
  const fin = Math.min(n, Math.ceil((top + H_VIEW) / H_ROW) + OVERSCAN);
  const vues = items.slice(debut, fin);
  return (
    <div className="proj-list" style={{ height: H_VIEW, overflowY: "auto" }}
      onScroll={(e) => setTop((e.target as HTMLDivElement).scrollTop)}>
      <div style={{ height: n * H_ROW, position: "relative" }}>
        <div style={{ position: "absolute", top: debut * H_ROW, left: 0, right: 0 }}>
          {vues.map((it, k) => (
            <Ligne key={it.label} it={it} total={total} rang={debut + k + 1}
              couleur={k % 2 ? couleur2 : couleur} actif={sel === it.label}
              onPick={() => onPick(it, debut + k + 1)} />
          ))}
        </div>
      </div>
    </div>
  );
}

export default function Projection({ onRun }: { onRun?: (query?: string) => void }) {
  const res = useAtomValue(queryResultAtom);
  const visible = useAtomValue(projectionVisibleAtom);
  const setVisible = useSetAtom(projectionVisibleAtom);
  const ancrer = useSetAtom(ancrerAtom);
  const refus = useAtomValue(projectionRefusAtom);
  const setRefus = useSetAtom(projectionRefusAtom);
  const [text, setText] = useAtom(queryTextAtom);
  const [axIdx, setAxIdx] = useState(0);
  const [filtre, setFiltre] = useState("");
  // Groupe choisi : ancrage ET part dans son axe (rang / pourcentage / effectif).
  const [sel, setSel] = useState<{ label: string; rang: number; it: ProjectItem } | null>(null);
  // « tout voir » : matérialisation EXPLICITE (jamais au rendu) de tous les groupes.
  const [tout, setTout] = useState<{ key: string; items: ProjectItem[]; ms: number } | null>(null);

  /**
   * Les axes ne sont PAS construits quand la projection est refusée pour cause de
   * volume : sinon le garde-fou ne protégerait que l'affichage, pas le calcul.
   * Le `try` est un filet : une exception ici remonterait dans l'arbre React
   * entier (`Inspector` n'est enveloppé d'aucun `CanvasBoundary`) — c'est
   * exactement ce que faisait l'ancien `Math.min(...vals)`.
   */
  const { axes, errProj } = useMemo(() => {
    if (!res || refus) return { axes: [] as ProjectAxis[], errProj: null as string | null };
    try { return { axes: projectAxes(res.columns || [], res.rows || []), errProj: null }; }
    catch (e: any) { return { axes: [] as ProjectAxis[], errProj: "projection impossible : " + String(e?.message || e) }; }
  }, [res, refus]);

  const axis = axes.length ? axes[Math.min(axIdx, axes.length - 1)] : undefined;
  // Changement d'axe : la sélection, le filtre et la matérialisation complète ne
  // veulent plus rien dire pour le nouvel axe.
  useEffect(() => { setSel(null); setFiltre(""); setTout(null); }, [axis?.key, res]);

  if (!visible) return null;   // fermée par l'utilisateur : rouverte par le ⌗
  const total = axis ? axisTotal(axis) : 0;

  const anchor = (qn: string) => {
    ancrer({ centers: [qn], label: qn });
  };
  const pick = (it: ProjectItem, rang: number) => {
    setSel({ label: it.label, rang, it });
    if (axis?.anchorable) anchor(it.label);
  };
  const toutVoir = () => {
    if (!axis || !res) return;
    const t0 = performance.now();
    const items = axisItemsFull(res.columns || [], res.rows || [], axis.key);
    setTout({ key: axis.key, items, ms: performance.now() - t0 });
  };
  // La liste AFFICHÉE : les 64 groupes construits, ou tous après « tout voir ».
  const source = tout && tout.key === axis?.key ? tout.items : (axis?.items || []);
  const q = filtre.trim().toLowerCase();
  const filtres = q ? source.filter((it) => it.label.toLowerCase().includes(q)) : source;
  const complets = !!tout && tout.key === axis?.key;

  return (
    <div className="proj">
      <div className="proj-head">
        <div className="proj-title">
          <span className="proj-dot" />
          <span>Projection</span>
        </div>
        <span className="proj-meta">
          {res?.execution_time_ms != null && <em>{res.execution_time_ms.toFixed(1)} ms</em>}
          <em>{(res?.rows || []).length} lignes</em>
        </span>
        <button className="ctl-x" aria-label="Fermer la projection" title="Fermer"
          onClick={() => setVisible(false)}>×</button>
      </div>

      {refus ? (
        // LE GARDE-FOU DE VOLUME — il DIT le nombre de lignes et laisse CHOISIR.
        <div className="proj-refus">
          <p>
            <b>{refus.rows.toLocaleString("fr-FR")} lignes</b> : projection non lancée d'office.
            Au-delà de {SEUIL_PROJECTION_ROWS.toLocaleString("fr-FR")} lignes, la construction des
            axes coûte plus cher que ce qu'elle montre (mesuré : ~25 ms à 50 000 lignes pour
            {" "}{TOP_GROUPES} barres ; l'ancien code levait une <i>RangeError</i> vers 125 000).
          </p>
          <div className="proj-refus-btns">
            <button className="btn" onClick={() => {
              // LA REQUÊTE EST RÉÉCRITE, PAS SEULEMENT RELANCÉE : borner à
              // l'exécution sans mettre à jour l'éditeur laisserait l'utilisateur
              // devant un texte qui ne dit pas ce qui a été exécuté (constaté :
              // l'éditeur gardait « LIMIT 25000 »).
              const q2 = withLimit(text, SEUIL_PROJECTION_ROWS);
              setText(q2);
              onRun?.(q2);
            }}>
              ajouter LIMIT {SEUIL_PROJECTION_ROWS}
            </button>
            <button className="textbtn" onClick={() => setRefus(null)}>projeter quand même</button>
          </div>
        </div>
      ) : errProj ? (
        <pre className="proj-error">{errProj}</pre>
      ) : res?.error ? (
        <pre className="proj-error">{res.error}</pre>
      ) : !res ? (
        // ÉTAT VIDE EXPLICITE (l'ancien `return null` ne disait rien).
        <p className="proj-empty">
          Aucune requête exécutée. Écrivez une requête dans le champ ci-dessus, ou partez d'un
          exemple, puis « ▶ Projeter » : la réponse sera organisée en axes (un axe par colonne,
          groupes agrégés, distribution des mesures).
        </p>
      ) : axes.length === 0 ? (
        <p className="proj-empty">Réponse vide — écrivez une requête (ex. types de nœuds).
        </p>
      ) : (
        <>
          <div className="proj-axes">
            {axes.map((a, i) => (
              <button key={a.key} className={"proj-ax" + (i === axIdx ? " on" : "")}
                title={`${a.kind === "cat" ? a.groupCount + " groupes" : a.groupCount + " intervalles"} · total ${a.total}`}
                onClick={() => setAxIdx(i)}>
                {a.key}
              </button>
            ))}
          </div>

          {axis?.kind === "cat" ? (
            <>
              {/* CE QUE LE PANNEAU NE MONTRE PAS — dit, chiffré, et ouvrable. */}
              <div className="proj-tell">
                <b>{axis.groupCount}</b> groupes · <b>{axis.items.length}</b> plus gros affichés
                {axis.hidden > 0 ? <> · <b>{axis.hidden}</b> masqués</> : " · tous affichés"}
                {!complets && axis.hidden > 0 && (
                  <button className="proj-plus" onClick={toutVoir}
                    title="Matérialise TOUS les groupes (coût mesuré : ~60 ms à 300 000 lignes) et les affiche en liste défilante fenêtrée">
                    tout voir ({axis.groupCount})
                  </button>
                )}
                {complets && (
                  <span className="proj-plus-info">
                    {tout!.items.length} groupes matérialisés en {tout!.ms.toFixed(0)} ms
                    <button className="textbtn" onClick={() => setTout(null)}>replier</button>
                  </span>
                )}
              </div>
              <div className="proj-filtre">
                <input value={filtre} onChange={(e) => setFiltre(e.target.value)}
                  placeholder={complets ? "filtrer les libellés (tous les groupes)" : "filtrer les libellés affichés"}
                  aria-label="Filtrer les libellés de l'axe" />
                {q && <span className="proj-filtre-n">{filtres.length} résultat(s){!complets && axis.hidden > 0 ? " — « tout voir » pour chercher dans la traîne" : ""}</span>}
              </div>

              {complets ? (
                <ListeFenetree items={filtres} total={total} couleur={AXIS_COLOR[0]}
                  couleur2={AXIS_COLOR[1]} sel={sel?.label ?? null}
                  onPick={(it) => pick(it, source.indexOf(it) + 1)} />
              ) : (
                <div className="proj-bars">
                  {filtres.map((it, i) => (
                    <Ligne key={it.label} it={it} total={total} rang={source.indexOf(it) + 1}
                      couleur={AXIS_COLOR[i % AXIS_COLOR.length]} actif={sel?.label === it.label}
                      onPick={() => pick(it, source.indexOf(it) + 1)} />
                  ))}
                </div>
              )}

              {sel && (
                <div className="proj-detail">
                  <b>{sel.label}</b> — rang <b>{sel.rang}</b>/{axis.groupCount} · part{" "}
                  <b>{total ? ((sel.it.value / total) * 100).toFixed(1) : "0"} %</b> · valeur{" "}
                  <b>{sel.it.value}</b>
                  {sel.it.cnt != null && <> · effectif <b>{sel.it.cnt}</b> ligne(s)</>}
                  {!axis.anchorable && <> · groupe non ancrable</>}
                </div>
              )}
            </>
          ) : axis?.kind === "num" ? (
            <div className="proj-bars">
              {(axis.values || []).map((it, i) => (
                <div key={it.label} className="proj-row">
                  <span className="proj-row-lbl">{it.label}</span>
                  <span className="proj-track">
                    <span className="proj-fill" style={{
                      width: `${total ? (it.value / total) * 100 : 0}%`,
                      background: AXIS_COLOR[0],
                    }} />
                  </span>
                  <span className="proj-row-val">{it.value}</span>
                </div>
              ))}
              {axis.min != null && axis.max != null && (
                <div className="proj-stats">
                  min {axis.min} · max {axis.max}
                  {axis.mean != null && <> · moyenne {axis.mean.toFixed(2)}</>}
                  {" · "}{axis.total} valeur(s) finie(s) en {axis.groupCount} intervalle(s)
                </div>
              )}
            </div>
          ) : null}

          {axis?.anchorable && (
            <div className="proj-note">● chaque groupe est un qualified_name — cliquez pour ancrer
              dans le graphe (la part du groupe s'affiche alors sous la liste).</div>
          )}
        </>
      )}
    </div>
  );
}
