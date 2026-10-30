import { useEffect, useMemo, useState } from "react";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import { selectedNodeAtom, lastGraphAtom, centersAtom, requestTokenAtom,
  layersAtom, overlayAtom,
  ancrerAtom,
} from "../app/store";
import type { GraphNode } from "../lib/api";
import {
  aIndexer, construireIndex, estFichier, fichierDe, fratrieCibleAtom, fratrieDe,
  type Fratrie, type GenreFratrie, type GroupeFratrie, type IndexFratrie,
} from "../graph/fratrie";

const short = (id: string) => id.split(".").slice(-1)[0];

/**
 * ── LA BARRE DE FACETTES : CE QUI ENTOURE L'ÉLÉMENT SÉLECTIONNÉ ─────────────
 *
 * DEMANDE : « qu'on voie bien les frères et les sœurs en cas de sélection d'un
 * élément, pour garantir l'affichage de l'immensité d'information. »
 *
 * ── CE QUI N'ALLAIT PAS ───────────────────────────────────────────────────
 *
 * 1. Les seuls voisins montrés étaient les arêtes `CALLS` PRÉSENTES dans le
 *    sous-graphe chargé. Deux fichiers du même dossier sans arête entre eux
 *    n'y apparaissaient donc JAMAIS — alors que la charge utile porte de quoi
 *    les relier : `file_path`, `community_id` et `scc_id` sont renvoyés par
 *    `/api/viz/subgraph` (voir `lib/api.ts`), et `scc_id` comme `file_path`
 *    n'étaient lus NULLE PART dans cette barre.
 * 2. La liste était coupée à 14 SANS LE DIRE (`f.items.slice(0, 14)`) : rien
 *    n'annonçait le total, donc rien ne permettait de savoir qu'il en manquait.
 *
 * Ici : la fratrie, les co-fichiers, les sœurs, le cycle et les homonymes
 * viennent d'un INDEX construit une seule fois par chargement (voir
 * `graph/fratrie.ts`) — pas d'un parcours d'arêtes, pas d'une requête. Chaque
 * relation ANNONCE son effectif réel, et le plafond d'affichage est visible et
 * levable.
 */

/** Plafond d'AFFICHAGE d'une relation — annoncé, jamais silencieux. */
const PLAFOND_AFFICHE = 14;

type Facet =
  | "fratrie" | "co-fichiers" | "soeurs" | "cycle" | "noms"
  | "parents" | "enfants" | "communaute" | "contour" | "energie" | "strates";

/**
 * ── LA CIBLE DE FRATRIE SURVIT À L'ANCRAGE ─────────────────────────────────
 *
 * Piège traité explicitement : `ancrerAtom` écrit `selectedNodeAtom = null`
 * (`src/app/store.ts`). Cliquer un frère fermait donc la barre à l'instant du
 * clic — impossible d'aller de frère en frère, qui est le geste même de la
 * demande. La cible de fratrie vit dans son propre atome (déclaré dans
 * `graph/fratrie.ts`) : elle n'est pas la sélection, elle est « la fratrie
 * ouverte », et elle se rouvre sur le nouveau nœud dès qu'il est chargé.
 */
export default function FacetBar() {
  const [sel, setSel] = useAtom(selectedNodeAtom);
  const [cible, setCible] = useAtom(fratrieCibleAtom);
  const last = useAtomValue(lastGraphAtom);
  const [open, setOpen] = useState<Facet | null>(null);
  const [deplie, setDeplie] = useState<Facet | null>(null);
  /**
   * LE DERNIER NŒUD DÉSIGNÉ PAR UN CLIC DE FRATRIE.
   *
   * `ancrer` vide la sélection et recharge un sous-graphe autour du nouveau
   * centre : pendant ce chargement, l'ancien graphe est encore à l'écran et le
   * nouveau nœud n'y figure pas. Sans cette mémoire, le panneau retomberait sur
   * un identifiant numérique — et le fait même d'avoir changé de cible ne se
   * lirait plus nulle part.
   */
  const [dernier, setDernier] = useState<{ id: string; qn: string; label: string } | null>(null);
  // UNE SEULE PORTE : poser un centre allonge le fil ET recharge. Avant, ce
  // composant écrivait `centersAtom` puis `bump()` à la main, sans toucher au
  // fil : le fil d'Ariane affichait donc un chemin que la vue n'avait pas suivi.
  const ancrer = useSetAtom(ancrerAtom);
  const bump = useSetAtom(requestTokenAtom);
  const setLayers = useSetAtom(layersAtom);
  const setOverlay = useSetAtom(overlayAtom);

  /**
   * L'INDEX DE PARENTÉ — construit UNE fois par chargement, HORS du chemin de
   * rendu. `setTimeout(0)` rend la main APRÈS la peinture : sur un chargement
   * de 200 000 nœuds, la construction (~134 ms mesurés) ne bloque donc pas
   * l'image qui vient d'afficher le graphe. Jamais dans une boucle d'animation.
   */
  const [index, setIndex] = useState<IndexFratrie | null>(null);
  useEffect(() => {
    if (!last) { setIndex(null); return; }
    let annule = false;
    const t = window.setTimeout(() => {
      if (annule) return;
      setIndex(construireIndex(aIndexer(last.nodes || [])));
    }, 0);
    return () => { annule = true; window.clearTimeout(t); };
  }, [last]);

  const parId = useMemo(() => {
    const m = new Map<string, GraphNode>();
    for (const n of last?.nodes || []) m.set(String(n.id), n);
    return m;
  }, [last]);

  // La sélection écrit la cible ; la cible, elle, survit à l'ancrage.
  useEffect(() => { if (sel) setCible(sel); }, [sel, setCible]);

  // Rien à montrer : ni sélection, ni fratrie ouverte.
  const nid = sel ?? cible;
  if (!nid) return null;
  const node: GraphNode | undefined = parId.get(nid);
  /**
   * L'ÉLÉMENT EST-IL DANS LE GRAPHE CHARGÉ ?
   *
   * Piège mesuré : `ancrer` recharge un sous-graphe AUTOUR du frère cliqué
   * (jusqu'à plusieurs secondes), pendant lesquelles `last` porte encore
   * l'ancien graphe. Répondre sur la fratrie d'une clé inconnue de l'index
   * inventerait une parenté (le repli dériverait le parent d'un identifiant
   * numérique). On le DIT donc, au lieu de fabriquer.
   */
  const connu = !!index && (index.qn.has(nid) || !!node?.qn && index.cle.has(String(node.qn)));
  const cleIdx = node?.qn && index?.cle.has(String(node.qn)) ? String(index.cle.get(String(node.qn))) : nid;
  const edges = last?.edges || [];
  const parents = [...new Set(edges.filter(e => e.dst === nid && e.type === "CALLS").map(e => e.src))];
  const enfants = [...new Set(edges.filter(e => e.src === nid && e.type === "CALLS").map(e => e.dst))];
  /**
   * ── NOMMER UN FRÈRE ─────────────────────────────────────────────────────
   *
   * Trois pièges, tous constatés à l'écran :
   *  1. le `label` du sous-graphe vaut le dernier segment du qn — donc
   *     « __file__ » pour tous les nœuds FICHIERS (mesuré : une liste de frères
   *     où chaque puce s'appelait « → __file__ ») ;
   *  2. le dernier segment utile du qn vaut « c » pour `…e1000_main.c.__file__`
   *     (le nom de fichier contient un point, la découpe le coupe) ;
   *  3. `file_path` existe sur TOUS les nœuds, pas seulement les fichiers : en
   *     faire le nom de tout le monde donnerait « e1000_hw.c » pour des
   *     fonctions — un nom qui ne distingue rien.
   * On ne prend donc le nom de fichier QUE si le nœud EST un fichier (son qn
   * porte le marqueur `__file__`), et le fichier réel fait foi.
   */
  const node2fichier = (id: string): string | undefined => {
    const n = parId.get(id);
    const q = String(n?.qn || index?.qn.get(id) || "");
    if (!q || !estFichier(q)) return undefined;
    const fp = n?.file_path;
    if (fp) return fp.split(/[/\\]/).pop();
    return fichierDe(q);
  };
  const name = (id: string) => node2fichier(id) || index?.nom.get(id) || parId.get(id)?.label || short(String(id));
  const hasRicci = edges.some(e => e.ricci != null && e.ricci < 0);
  const hasWeight = edges.some(e => (e.weight ?? 0) > 0 || (e.pheromone ?? 0) > 0);

  const fr: Fratrie | null = index && connu ? fratrieDe(index, cleIdx) : null;
  const groupe = (g: GenreFratrie): GroupeFratrie | undefined =>
    fr?.groupes.find(x => x.genre === g);

  const fermer = () => { setSel(null); setCible(null); setOpen(null); };
  const anchor = (id: string) => {
    // ── LE PIÈGE `ancrerAtom`, TRAITÉ ICI ────────────────────────────────────
    // `ancrerAtom` (`src/app/store.ts`) écrit `selectedNodeAtom = null` : c'est
    // VOULU (le centre précédent ne se sélectionne plus), mais `App` ne monte
    // `FacetBar` QUE si `selectedNode` est non nul. Cliquer un frère fermait
    // donc la barre à l'instant du clic, et l'on ne pouvait pas aller de frère
    // en frère — le geste même de la demande.
    //
    // On repose donc la sélection sur le frère CHOISI, dans le même tour : le
    // clic dit « c'est lui que je regarde ». La cible de fratrie, elle, est
    // posée AVANT l'ancrage (elle seule survit à un rechargement, et elle
    // permet au panneau de NOMMER le nouveau nœud avant l'arrivée du graphe).
    const libelle = name(id);
    setCible(id);
    setDernier({ id, qn: index?.qn.get(id) || String(id), label: libelle });
    ancrer({ centers: [id], label: libelle });
    setSel(id);
    setDeplie(null);
  };
  const activate = (f: Facet) => {
    if (f === "communaute") setLayers(l => ({ ...l, spectral: true }));
    else if (f === "contour") setOverlay("ricci");
    else if (f === "energie") setOverlay("energy");
    else if (f === "strates") setLayers(l => ({ ...l, synapse: true, k4: true }));
    setOpen(o => (o === f ? null : f));
  };

  /** Ligne d'état d'une RELATION : effectif réel, clé réelle, raison si vide. */
  const ligne = (g: GroupeFratrie | undefined, quoi: string): string => {
    if (!connu) return `nœud « ${dernier?.id === nid ? dernier.label : nid} » pas encore chargé — rechargement en cours`;
    if (!g) return "index de parenté en construction…";
    if (!g.total) return g.raison || "aucun";
    const cle = g.cle && g.cle !== "?" ? ` « ${g.cle} »` : "";
    return `${g.total} ${quoi}${cle} · ${fr?.indexes ?? 0} nœuds chargés au total`;
  };

  /**
   * Les puces d'une relation : le nom, et un dépliage VISIBLE au-delà du
   * plafond. Avant, `slice(0, 14)` coupait sans rien dire.
   */
  const puces = (g: GroupeFratrie | undefined, cleFacet: Facet) => {
    if (!connu) return <span className="chip">{`en attente du chargement de ${dernier?.id === nid ? dernier.label : nid}`}</span>;
    if (!g || !g.total) return <span className="chip">{g?.raison || "index en construction…"}</span>;
    const tout = deplie === cleFacet;
    const vus = tout ? g.membres : g.membres.slice(0, PLAFOND_AFFICHE);
    const reste = g.total - vus.length;
    return (
      <>
        {vus.map(id => (
          <button key={id} className="chip" title={`${index?.qn.get(id) || id} — sélectionner et cadrer`}
            onClick={() => anchor(id)}>→ {name(id)}</button>
        ))}
        {!tout && reste > 0 && (
          <button className="chip" title={`Afficher les ${g.total} — aucune troncature silencieuse`}
            onClick={() => setDeplie(cleFacet)}>+{reste} autres</button>
        )}
        {tout && g.total > PLAFOND_AFFICHE && (
          <button className="chip" onClick={() => setDeplie(null)}>replier</button>
        )}
      </>
    );
  };

  const list: { k: Facet; label: string; n?: number; dis?: boolean; raison?: string }[] = [
    { k: "fratrie", label: "Frères", n: groupe("freres")?.total,
      dis: !groupe("freres")?.total, raison: !connu ? "nœud pas encore chargé" : groupe("freres")?.raison },
    { k: "co-fichiers", label: "Co-fichiers", n: groupe("co-fichiers")?.total,
      dis: !groupe("co-fichiers")?.total, raison: groupe("co-fichiers")?.raison },
    { k: "soeurs", label: "Sœurs", n: groupe("soeurs")?.total,
      dis: !groupe("soeurs")?.total, raison: groupe("soeurs")?.raison },
    { k: "cycle", label: "Cycle", n: groupe("cycle")?.total,
      dis: !groupe("cycle")?.total, raison: groupe("cycle")?.raison },
    { k: "noms", label: "Homonymes", n: groupe("homonymes")?.total,
      dis: !groupe("homonymes")?.total, raison: groupe("homonymes")?.raison },
    // ── relations par FLUX (arêtes CALLS du sous-graphe chargé) ────────────
    { k: "parents", label: "Appelé par", n: parents.length, dis: !parents.length, raison: "aucune arête CALLS entrante chargée" },
    { k: "enfants", label: "Appelle", n: enfants.length, dis: !enfants.length, raison: "aucune arête CALLS sortante chargée" },
    { k: "communaute", label: "Communauté",
      dis: node?.community_id == null, raison: "pas de communauté" },
    { k: "contour", label: "∂ Contour", dis: !hasRicci, raison: "pas de ricci" },
    { k: "energie", label: "Énergie", dis: node?.energy == null, raison: "pas d'énergie" },
    { k: "strates", label: "Strates", dis: !hasWeight, raison: "pas de poids" },
  ];

  return <div className="facetbar">
    <div className="facetbar-head">
      <b>{node?.label || (dernier?.id === nid ? dernier.label : name(nid))}</b>
      <span className="facetbar-qn">{node?.qn || (dernier?.id === nid ? dernier.qn : index?.qn.get(nid)) || nid}</span>
      {node?.file_path && <span className="facetbar-file">{node.file_path}</span>}
      {/* OÙ L'ÉLÉMENT SE SITUE : le dossier porteur, le module, le fichier —
          trois relations réelles, dérivées d'une seule règle (`parentDe`). */}
      {fr && (
        <span className="facetbar-situe" style={{ opacity: 0.8 }}>
          dossier <b>{fr.parent}</b> · module <b>{fr.module}</b>
          {fr.fichier ? <> · fichier <b>{fr.fichier}</b></> : null}
          {fr.communaute !== undefined ? <> · communauté <b>{fr.communaute}</b></> : null}
          {fr.scc !== undefined ? <> · scc <b>{fr.scc}</b></> : null}
        </span>
      )}
      {!connu && <span className="facetbar-situe" style={{ opacity: 0.7 }}>rechargement autour de ce frère…</span>}
      <button className="ctl-x" aria-label="Fermer" onClick={fermer}>×</button>
    </div>
    {/* ── LE TOTAL, TOUJOURS ÉCRIT ───────────────────────────────────────────
        La demande est « qu'on voie bien les frères et les sœurs ». Une
        sélection qui n'afficherait les effectifs qu'APRÈS avoir déplié une
        facette cacherait justement ce qu'on veut voir. Ici, les cinq relations
        et le dénominateur sont annoncés d'emblée. */}
    <div className="facetbar-totaux" style={{ display: "flex", flexWrap: "wrap", gap: 4, fontSize: 11, lineHeight: 1.35 }}>
      <span className="qb-lbl">
        {!index ? "index de parenté en construction…"
          : !connu ? `fratrie : nœud « ${dernier?.id === nid ? dernier.label : nid} » pas encore chargé (${index.n} nœuds indexés)`
          : <>fratrie de <b>{name(nid)}</b> — dossier « {fr?.parent} » :{" "}
            <b>{groupe("freres")?.total ?? 0}</b> frères · <b>{groupe("co-fichiers")?.total ?? 0}</b> co-fichiers ·{" "}
            <b>{groupe("soeurs")?.total ?? 0}</b> sœurs · <b>{groupe("cycle")?.total ?? 0}</b> dans le même cycle ·{" "}
            <b>{groupe("homonymes")?.total ?? 0}</b> homonymes — sur <b>{index.n}</b> nœuds chargés
            {index.ms ? ` (index en ${index.ms} ms, sans requête)` : ""}</>}
      </span>
    </div>
    <div className="facetbar-facets">
      {list.map(f => (
        <span key={f.k} className="facet-grp">
          <button className={"chip" + (open === f.k ? " on" : "")}
            disabled={!!f.dis} title={f.dis ? f.raison : `${f.label} — ${f.n ?? "?"}`}
            onClick={() => activate(f.k)}>
            {f.label}{typeof f.n === "number" ? <> <i>{f.n}</i></> : null}
          </button>
          {open === f.k && !f.dis && (
            <span className="facet-items">
              {/* Le TOTAL est écrit même quand on tronque l'affichage. */}
              <span className="qb-lbl" style={{ width: "100%" }}>
                {f.k === "fratrie" ? ligne(groupe("freres"), "frères (même dossier porteur)")
                  : f.k === "co-fichiers" ? ligne(groupe("co-fichiers"), "nœuds dans le même fichier")
                  : f.k === "soeurs" ? ligne(groupe("soeurs"), "sœurs (même communauté Leiden)")
                  : f.k === "cycle" ? ligne(groupe("cycle"), "membres de la même composante (scc)")
                  : f.k === "noms" ? ligne(groupe("homonymes"), "homonymes (même nom, autre dossier)")
                  : null}
              </span>
              {f.k === "fratrie" ? puces(groupe("freres"), "fratrie")
                : f.k === "co-fichiers" ? puces(groupe("co-fichiers"), "co-fichiers")
                : f.k === "soeurs" ? puces(groupe("soeurs"), "soeurs")
                : f.k === "cycle" ? puces(groupe("cycle"), "cycle")
                : f.k === "noms" ? puces(groupe("homonymes"), "noms")
                : f.k === "parents" || f.k === "enfants" ? (
                  <>
                    {(f.k === "parents" ? parents : enfants).map(it =>
                      <button key={it} className="chip" onClick={() => anchor(it)}>→ {name(it)}</button>)}
                  </>
                ) : f.k === "communaute" ? <span className="chip">{`communauté ${node?.community_id}`}</span>
                : f.k === "contour" ? <span className="chip">arêtes ∂ (ricci&lt;0)</span>
                : f.k === "energie" ? <span className="chip">{`énergie ${node?.energy}`}</span>
                : <span className="chip">synapses pondérées</span>}
            </span>
          )}
        </span>
      ))}
    </div>
  </div>;
}
