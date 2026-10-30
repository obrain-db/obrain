import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { PanneauGens } from "../graph/PanneauGens";
import { PanneauMonde } from "../graph/PanneauMonde";
import { PanneauFlotte } from "../graph/PanneauFlotte";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import {
  storesAtom, activeStoreAtom, centersAtom, hopsAtom, overlayAtom, limitAtom,
  layersAtom, statsAtom, requestTokenAtom, historyAtom, lastGraphAtom,
  selectedNodeAtom, suggestionsAtom, organicModeAtom, morphModeAtom,
  inspectorOpenAtom, queryTextAtom, queryResultAtom, queryBusyAtom,
  projectionVisibleAtom, projectionRefusAtom, type Anchor,
  faceAtom, niveauDemandeAtom, niveauAfficheAtom, ancrerAtom, rechargerAtom,
  ecranAtom, chromeReduitAtom, montreAtom, canvasInsetAtom, type NiveauCosmos,
  subjectAtom, chatOuvertAtom,
} from "./store";
import type { GraphEdge } from "../lib/api";
import { fetchStores, runQuery, resoudreAncres } from "../lib/api";
import { refusProjection } from "../lib/projection";
import CommandBar from "../components/CommandBar";
import Inspector from "../components/Inspector";
import FacetBar from "../components/FacetBar";
import StageContinu from "../components/StageContinu";
import CosmosWeave from "../graph/CosmosWeave";
import { ChatPleinEcran } from "../graph/ChatPleinEcran";
import TimelineView from "../graph/TimelineView";
import { fetchOverview } from "../lib/api";
import CanvasBoundary from "../components/CanvasBoundary";
import Graph3D from "../graph/Graph3D";
import StoreOverview from "../components/StoreOverview";

const short = (id: string) => id.split(".").slice(-1)[0];
function runtimeLike(label: string): boolean {
  const l = (label || "").toLowerCase();
  return l.includes("runtime") || l.includes("self") || l.includes("engram");
}
function defaultsFor(label: string): { centers: string[]; hops: number; limit: number } {
  const l = (label || "").toLowerCase();
  if (l.includes("complet")) return { centers: ["tcp_v4_rcv"], hops: 2, limit: 900 };
  if (l.includes("linux")) return { centers: ["e1000_probe"], hops: 1, limit: 160 };
  return { centers: ["substitute_in_operator"], hops: 1, limit: 160 };
}

export default function App() {
  const [ctlOpen, setCtlOpen] = useAtom(inspectorOpenAtom);
  const [cmd, setCmd] = useState("");
  const [askMsg, setAskMsg] = useState("");
  const [info, setInfo] = useAtom(storesAtom);
  const [active, setActive] = useAtom(activeStoreAtom);
  const [centers, setCenters] = useAtom(centersAtom);
  const [hops, setHops] = useAtom(hopsAtom);
  const [overlay, setOverlay] = useAtom(overlayAtom);
  const [limit, setLimit] = useAtom(limitAtom);
  const [layers, setLayers] = useAtom(layersAtom);
  const [history, setHistory] = useAtom(historyAtom);
  const [selected, setSelected] = useAtom(selectedNodeAtom);
  const [sug, setSug] = useAtom(suggestionsAtom);
  // LA FACE : un seul atom pour la choisir, des booléens DÉRIVÉS pour la lire.
  const [face, setFace] = useAtom(faceAtom);
  const cosmos = face === "cosmos";
  const monde = face === "monde";
  const [morph, setMorph] = useAtom(morphModeAtom);
  // ── LA SURFACE D'ÉCRAN, EN UNE SEULE GRANDEUR ─────────────────────────────
  // Un seul écouteur écrit `ecranAtom`; tout ce qui s'adapte (surface des menus,
  // cibles tactiles, ce qui disparaît) s'en DÉDUIT. Aucun composant ne mesure la
  // fenêtre de son côté : c'est ainsi qu'on obtenait des affichages incohérents.
  const ecran = useAtomValue(ecranAtom);
  const setEcran = useSetAtom(ecranAtom);
  const [reduit, setReduit] = useAtom(chromeReduitAtom);
  const montre = useAtomValue(montreAtom);
  const inset = useAtomValue(canvasInsetAtom);
  // UN SEUL écrivain de la classe d'écran : deux seuils dans deux composants,
  // c'est ainsi qu'on obtenait un panneau replié sur un écran qui ne l'est plus.
  useEffect(() => {
    // (2026-09-15, mobile praticable) Seuils REELS : les telephones actuels
    // rendent 390-430 px CSS, mais paysage et grands modeles montent a ~700 ;
    // 480 laissait un vrai telephone en mode « tablette », chrome complet,
    // ecran mange. Et sur telephone le chrome demarre REDUIT : un ecran a la
    // fois — la navigation, le chat, la scene. Le geste d agrandir reste.
    let ecranPrecedent = "";
    const lire = () => {
      const e = window.innerWidth <= 700 ? "telephone"
        : window.innerWidth <= 1024 ? "tablette" : "bureau";
      setEcran(e);
      if (e === "telephone" && ecranPrecedent !== "telephone") setReduit(true);
      ecranPrecedent = e;
    };
    lire();
    window.addEventListener("resize", lire);
    window.addEventListener("orientationchange", lire);
    return () => {
      window.removeEventListener("resize", lire);
      window.removeEventListener("orientationchange", lire);
    };
  }, [setEcran, setReduit]);
  const [fx, setFx] = useState(""); // zoom-out / zoom-in entre échelles
  const [fxOrigin, setFxOrigin] = useState("50% 50%");
  const lastPt = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  const [organic, setOrganic] = useAtom(organicModeAtom);
  const [, setQueryText] = useAtom(queryTextAtom);
  const setQueryRes = useSetAtom(queryResultAtom);
  const setQueryBusy = useSetAtom(queryBusyAtom);
  const projVisible = useAtomValue(projectionVisibleAtom);
  const setProjVisible = useSetAtom(projectionVisibleAtom);
  const setProjRefus = useSetAtom(projectionRefusAtom);
  const last = useAtomValue(lastGraphAtom);
  const stats = useAtomValue(statsAtom);
  // L'ANCRAGE : une seule porte (périmètre + fil + rechargement), et un
  // rechargement nu pour ce qui ne change qu'une couche d'affichage.
  const ancrer = useSetAtom(ancrerAtom);
  const recharger = useSetAtom(rechargerAtom);
  const queryCtl = useRef<AbortController | null>(null);
  useEffect(() => () => queryCtl.current?.abort(), []);

  useEffect(() => {
    fetchStores().then((j: any) => {
      setInfo(j);
      if (j?.presets?.length && !active) {
        // URL « pages » : ?store=<label|path> force le store (dive géré par le Monde)
        const params = new URLSearchParams(window.location.search);
        const want = params.get("store");
        const def = (want && j.presets.find((p: any) => p.label === want || p.path === want))
          || j.presets.find((p: any) => p.label === j.default) || j.presets[0];
        setActive(def.path);
        // Page « monde » via ?dive=… (sous-espace géré par StoreOverview)
        if (params.get("dive")) { setFace("monde"); setCenters([]); return; }
        if (params.get("cosmos")) { setFace("cosmos"); return; }
        // ── LES DÉFAUTS SONT CEUX DU STORE DEMANDÉ (2026-09-13) ──────────────
        // DÉFAUT MESURÉ : `defaultsFor(j.default)` lisait le magasin PAR DÉFAUT du
        // serveur (« Runtime dev (engrams) ») au lieu de celui que l'URL demande.
        // Avec `?store=Linux e1000`, la page posait donc l'ancre
        // `substitute_in_operator` — un symbole Rust — dans un store Linux :
        // mesuré, `/api/viz/subgraph` répondait 400 et la scène restait vide, sans
        // rien dire. On prend les défauts du store RÉELLEMENT activé.
        const d = defaultsFor(String(def.label || j.default));
        if (runtimeLike(String(def.label))) { setFace("monde"); setCenters([]); setHops(1); return; }
        // ── L'ANCRE EST UN NOM NU : LA RÉSOUDRE EN qn (2026-09-15) ───────────
        // DÉFAUT MESURÉ sur le 8432 : `e1000_probe` envoyé tel quel vaut
        // `400 {"error":"centre introuvable: e1000_probe"}` — la lignée servie
        // résout en STRICT (id ou qn exact), celle du tronc a un repli par
        // suffixe. Conséquence visible : choisir « Linux e1000 », « Linux
        // complet » ou « Obrain (rust) » laissait la scène VIDE, sans rien dire.
        // Voir `resoudreAncres` (api.ts) : le serveur reste strict, l'ancre
        // devient exacte, et un binaire au repli permissif résout encore.
        resoudreAncres(def.path, d.centers).then((ancres) => {
          setCenters(ancres); setHops(d.hops); setLimit(d.limit);
          setHistory([{ centers: ancres, hops: d.hops, label: d.centers[0] }]);
        });
      }
    }).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const pushAnchor = (next: Anchor) => {
    // Le périmètre, le fil et le rechargement partent d'un seul geste : il n'y a
    // plus de `bump()` à oublier, ni de fil qui montre un chemin non suivi.
    ancrer({ centers: next.centers, hops: next.hops, label: next.label });
    setCtlOpen(false); // la représentation redevient maîtresse après un ancrage
  };
  const goAnchor = (idx: number) => {
    const h = history[idx];
    setHistory((hh) => hh.slice(0, idx + 1));
    ancrer({ centers: h.centers, hops: h.hops, label: h.label, empiler: false });
  };
  const anchorOne = (qn: string) => pushAnchor({ centers: [qn], hops: Math.max(1, hops), label: qn });
  const setL = (k: string) => (v: boolean) => setLayers({ ...layers, [k]: v });

  const presets = info.presets || [];
  const activeLabel = presets.find((p: any) => p.path === active)?.label || active;

  /* ── LE CHAT N'EST PAS DANS LE CANVAS (2026-09-15) ───────────────────────
   *
   * Défaut mesuré, rapporté par l'utilisateur : « la vue des comptes arrive sur
   * le champ de saisie du message » et « les interfaces de chat doivent être
   * libérées de la charge GPU du canvas ».
   *
   * La cause était une seule ligne, et elle expliquait les deux symptômes :
   * `ChatPleinEcran` était monté par `CosmosWeave` — donc DANS la scène WebGL.
   * Conséquences, toutes deux structurelles :
   *   · son panneau plein écran vivait dans le même conteneur que la vue, donc
   *     il se posait SOUS les éléments montés à la racine (`AdminComptes`, le
   *     bandeau « aucune base ») : la vue des comptes recouvrait la saisie ;
   *   · ses re-rendus suivaient ceux du canvas (mouvement, focus, densité,
   *     niveaux) : le chat payait la charge GPU de la scène, et une scène qui
   *     échoue emportait la conversation avec elle.
   *
   * Un chat n'a rien à faire dans une scène 3D. Il est ici, au niveau de
   * l'application : au-dessus de la vue, indépendant de son rendu, et capable
   * d'évoluer (historique, comptes, résonance) sans toucher au canvas.
   */
  // L'ouverture du chat est un état PARTAGÉ (voir `chatOuvertAtom`) : le
  // panneau du canvas le demande, l'application le détient.
  const [chatOuvert, setChatOuvert] = useAtom(chatOuvertAtom);
  const [sujetChat] = useAtom(subjectAtom);
  // Timeline d'une session runtime : la session à inspecter (id entré par
  // l'utilisateur) et l'ouverture du panneau plein écran.
  const [tlOuvert, setTlOuvert] = useState(false);
  const [tlSession, setTlSession] = useState("");
  // La base visée par la conversation : celle que le SUJET porte quand il y en a
  // une (le serveur l'a résolue), sinon celle qui est ouverte. Le chat ne
  // devine rien : il envoie au serveur ce que la vue regarde.
  const storeChat = sujetChat?.store || active || "";

  /* PENDANT LA CONVERSATION, LA NAVIGATION DE VUE S'EFFACE.
   *
   * Défaut mesuré (390×844) : la barre d'onglets des vues est fixée EN BAS — les
   * onglets « 🌌 Cosmos / 🧠 Monde / 🔺 Groupe / 🧬 Signaux » se posaient SUR le
   * champ de saisie du message (4 contrôles recouvrant le champ, mesuré par
   * `scripts/ui-chat-porte.mjs`). C'est le défaut rapporté tel quel.
   *
   * Le choix : on ne déplace pas le champ, on retire la barre. Une conversation
   * plein écran qui garde une barre d'onglets flottante au-dessus de sa saisie
   * n'est pas plein écran — et l'utilisateur qui écrit n'a pas besoin de changer
   * de vue à cet instant. Le drapeau est posé sur la RACINE (une classe), donc
   * le CSS décide, sans que le chat ait à connaître la barre — ni l'inverse.
   */
  /* ── LE DRAPEAU EST DÉCLARATIF, PAS UN EFFET (2026-09-15) ────────────────
   *
   * Première version : un `useEffect` posait `chat-ouvert` sur la racine. Il
   * S'EXÉCUTAIT (« toggle chat-ouvert true » mesuré), et la classe n'était
   * pourtant JAMAIS présente : `React.StrictMode` monte, démonte et remonte
   * chaque composant en développement, donc l'effet était exécuté, son nettoyage
   * aussitôt appliqué (`remove`), puis réexécuté — et la trace montrait la classe
   * absente alors que les appels avaient bien eu lieu. Un premier correctif par
   * référence (`useRef`) n'a rien changé : le problème n'était pas de TROUVER la
   * racine, mais de faire dépendre un état d'un effet.
   *
   * Un drapeau qui décrit l'état de l'interface appartient au rendu : il est donc
   * dans le `className`. React le pose à chaque rendu, StrictMode n'a plus rien à
   * annuler, et le CSS fait le reste.
   */
  const goPreset = (label: string) => {
    const p = presets.find((x: any) => x.label === label);
    if (p) setActive(p.path);
    switchStore(label);
  };
  const switchStore = (label: string) => {
    queryCtl.current?.abort();
    // base cognitive/runtime : pas d'ancre de code — on ouvre sur le stage
    // (Monde = zoom dans sa bulle), jamais un centre inexistant.
    if (runtimeLike(label)) {
      setFace("monde"); setCenters([]); setHops(1);
      setSelected(null); setSug([]);
      ancrer(null); // vide le périmètre ET le fil, et recharge
      return;
    }
    const d = defaultsFor(label);
    setCenters(d.centers); setHops(d.hops); setLimit(d.limit);
    setSelected(null); setSug([]);
    setHistory([{ centers: d.centers, hops: d.hops, label: d.centers[0] }]);
    recharger();
  };

  // ── interprétation de la commande (naturel / GQL / symbole) ──
  const runGraphQuery = async (text: string) => {
    if (!active || !text.trim()) return;
    queryCtl.current?.abort();
    const ctl = new AbortController();
    queryCtl.current = ctl;
    setQueryBusy(true); setProjVisible(true); setCtlOpen(true);
    setQueryText(text);
    try {
      const r = await runQuery(active, text.trim(), "gql", ctl.signal);
      setQueryRes(r);
      // LE MÊME GARDE-FOU QU'AU BOUTON « PROJETER » : le serveur ne borne pas
      // `/api/query` (il sérialise `r.rows` tel quel) et la barre de commandes peut
      // donc demander 413 985 lignes aussi facilement que l'inspecteur.
      setProjRefus(refusProjection((r?.rows || []).length));
    }
    catch (e: any) {
      if (e?.name !== "AbortError") {
        setQueryRes({ columns: [], rows: [], error: String(e.message || e) });
        setProjRefus(null);
      }
    }
    finally { setQueryBusy(false); }
  };
  const ask = (raw: string) => {
    const q = raw.trim().toLowerCase(); if (!q) return;
    const st = raw.trim();
    // requête moteur explicite (GQL / Cypher) → exécution + projection
    if (/^(match|explain|call|select|return)\b/i.test(st) || /\breturn\b/i.test(st)) {
      runGraphQuery(st);
      setAskMsg("");
      return;
    }
    const toggleOverlay = (ov: string, msg: string) => {
      const next = overlay === ov ? "" : ov; setOverlay(next);
      setAskMsg(next ? msg : "Couche désactivée"); recharger();
    };
    if (q.includes("engram") || q.includes("mémoire") || q.includes("memoire")) {
      setOrganic(true); setAskMsg("Engrams/activité → vue Organique (halos hubs) activée"); recharger(); return; }
    if (q.includes("synapse") || q.includes("poids")) { setL("synapse")(!layers.synapse);
      setAskMsg(layers.synapse ? "Synapses masquées" : "Synapses (poids hebbiens) affichées"); return; }
    if (q.includes("communaut") || q.includes("spectral")) { setL("spectral")(!layers.spectral);
      setAskMsg(layers.spectral ? "" : "Communautés (spectral) colorées"); return; }
    if (q.includes("energie") || q.includes("énergie")) { toggleOverlay("energy", "Overlay Énergie : activité des nœuds"); return; }
    if (q.includes("ricci") || q.includes("courbure") || q.includes("contour")) { toggleOverlay("ricci", "Overlay Ricci : courbure + ∂ contours"); return; }
    if (q.includes("struct") || q.includes("k4") || q.includes("hyper")) { setL("k4")(true); setL("hyper")(true);
      setAskMsg("Strates : hyperfaces K4 + hyperarêtes"); return; }
    if (q.includes("appelle") || q.includes("caller") || q.includes("parent")) {
      setAskMsg("Parents/appelants : cliquez un nœud puis ← Parents"); return; }
    const m = q.match(/[a-z_][a-z0-9_]{2,}\\.?[a-z0-9_]*$/i);
    if (m) { const t = m[0]; anchorOne(t); setAskMsg("Cible → " + t); return; }
    setAskMsg("Interprétation ? Essayez : qui appelle, engrams, communautés, tcp_v4_rcv");
  };

  const rel = useMemo(() => {
    const edges: GraphEdge[] = last?.edges || [];
    const children = (id: string) => [...new Set(edges.filter((e) => e.src === id && e.type === "CALLS").map((e) => e.dst))];
    const parents = (id: string) => [...new Set(edges.filter((e) => e.dst === id && e.type === "CALLS").map((e) => e.src))];
    return { children, parents };
  }, [last]);
  const selectedNode = selected ? last?.nodes.find((n) => n.id === selected) : undefined;

  const fxTimers = useRef<number[]>([]);
  /** Transition directionnelle : « out » = on REMONTE vers le Cosmos
   *  (la projection se replie) ; « in » = on ENTRE dans le détail (elle
   *  grossit et s'efface, la nouvelle tombe en place depuis un plan large). */
  const zoomTo = (next: () => void, dir: "out" | "in" = "out") => {
    fxTimers.current.forEach(t => window.clearTimeout(t));
    fxTimers.current = [];
    setFxOrigin(`${lastPt.current.x}px ${lastPt.current.y}px`);
    setFx(dir === "in" ? "fwd-out" : "out");
    fxTimers.current.push(window.setTimeout(() => { next(); setFx(dir === "in" ? "fwd-in" : "in"); }, 200));
    fxTimers.current.push(window.setTimeout(() => setFx(""), 640));
    fxTimers.current.push(window.setTimeout(() => setFx(""), 1000)); // filet
  };
  /**
   * ── DEUX ÉTATS, ET NON UN SEUL : C'EST CE QUI CASSAIT LE ZOOM ──────────────
   *
   * Bug rapporté : « j'avance et je repars instantanément en arrière ».
   * Cause : UN SEUL état servait à la fois de DEMANDE et d'AFFICHAGE. La scène
   * rapportait son niveau → `setNiveauCosmos` → la prop `niveau` changeait → la
   * boucle croyait à une NOUVELLE DEMANDE et relançait le vol → la caméra
   * revenait à sa distance. En zoomant, on modifiait le niveau rapporté, donc on
   * redéclenchait le vol : un asservissement qui RAMÈNE la caméra en arrière.
   *
   *   niveauCosmos  = la DEMANDE  (posée UNIQUEMENT par les boutons)
   *   niveauAffiche = ce que la SCÈNE montre (posé par la scène, pour les boutons)
   *
   * Les rapports ne redeviennent donc jamais des demandes : la boucle se ferme.
   */
  // La DEMANDE et le RAPPORT sont deux atoms PARTAGÉS (et non deux `useState`) :
  // le fil des poupées, le sélecteur de vues et l'inspecteur lisent donc la même
  // vérité. Le commentaire ci-dessus explique pourquoi ils restent deux.
  const [niveauCosmos, setNiveauCosmos] = useAtom(niveauDemandeAtom);
  const [niveauAffiche, setNiveauAffiche] = useAtom(niveauAfficheAtom);
  /**
   * ── CALLBACKS STABLES : L'EFFET DE LA SCÈNE SE RELANÇAIT À CHAQUE RENDU ────
   *
   * Bug rapporté : « j'avance et je repars instantanément en arrière », reproduit
   * et instrumenté. La cause est ici, et elle est de moi :
   *
   *   - je fais remonter le niveau réel de la scène (`onNiveau` -> setNiveauAffiche) ;
   *   - en zoomant, ce niveau change, donc App SE RE-REND ;
   *   - `onOpen` était une fonction FLÈCHE recréée à chaque rendu, et le grand
   *     effet de la scène l'a dans ses dépendances `[weave, layout, onOpen, …]` ;
   *   - l'effet se relançait donc INTÉGRALEMENT : les intérieurs étaient
   *     reconstruits (les ensembles disparaissaient, `pret` retombait à faux) et
   *     la descente repartait de zéro.
   *
   * MESURÉ, cible inchangée pendant tout le zoom (donc ce n'était pas elle) :
   *     t : 0,18 -> 0,34 puis 0,000 ; boules : 6 -> 0
   * Et dix secondes SANS interaction : t, apparent, pret, boules parfaitement
   * stables — le déclencheur est bien l'interaction.
   *
   * On stabilise donc les identités : un rendu ne doit pas relancer une scène.
   */
  const ouvrirBase = useCallback((path: string, label: string) => {
    setActive(path);
    void label;
  }, []);
  const rapporterNiveau = useCallback((n: NiveauCosmos) => setNiveauAffiche(n), []);
  // (2026-09-15) AUCUNE BASE ACCORDEE : on ne monte AUCUN canvas. Le Cosmos
  // partirait sinon interroger le store PAR DEFAUT du serveur — une base que
  // ce compte n a pas le droit de lire — et l ecran d erreur 403 remplacait
  // la scene (constate avec le compte vierge achaptel). Le panneau
  // « aucune base » explique et donne la sortie ; c est lui, l ecran.
  const aucuneBase = !!(info && (info as any).compte && !(((info as any).presets || []).length));
  /**
   * ── LA MATIÈRE DE L'ESPACE ACTIF — POUR QUE LE VIDE S'EXPLIQUE ─────────────
   *
   * MESURÉ sur un compte neuf (self `achaptal` : 2 nœuds, 1 arête) : aucune
   * erreur, mais AUCUN MOT non plus — un espace accordé et vide ressemble
   * exactement à une application en panne. Le nombre vient du serveur
   * (`/api/viz/overview`, mis en cache par la couche réseau) ; en dessous du
   * seuil, l'écran doit dire ce qu'il contient ET donner la sortie.
   */
  const [matiere, setMatiere] = useState<{ nodes: number; edges: number } | null>(null);
  useEffect(() => {
    if (!active || aucuneBase) { setMatiere(null); return; }
    let vivant = true;
    fetchOverview(active)
      .then((o) => { if (vivant) setMatiere({ nodes: Number(o?.nodes ?? 0), edges: Number(o?.edges ?? 0) }); })
      .catch(() => { if (vivant) setMatiere(null); });   // on ne conclut pas « vide » sur un échec
    return () => { vivant = false; };
  }, [active, aucuneBase]);
  /** En dessous de ce nombre de nœuds, il n'y a rien à explorer — on le dit. */
  const SEUIL_VIDE = 10;
  const viewIdx = cosmos ? niveauAffiche : monde ? 1 : 2;
  /**
   * ── LES DIMENSIONS SONT DES ÉCHELLES, PAS DES PAGES ─────────────────────────
   *
   * `goView` REMPLAÇAIT le canevas (`setCosmos`/`setMonde`) en l'enveloppant d'un
   * `zoomTo` — un effet CSS cosmétique. Mesuré : cliquer « Monde » quittait la
   * scène Cosmos, et les étages (t, ouv, apparent) cessaient simplement d'être
   * mis à jour. Le voyage d'échelle traversait donc une frontière de canevas.
   *
   * Désormais : DANS le Cosmos, les dimensions DESCENDENT la même scène (niveau 0
   * cosmos, 1 monde, 2 groupe, 3 signaux). Hors du Cosmos, on y revient, puis on
   * descend — le changement de canevas ne sert plus qu'à rentrer.
   */
  const goView = (t: number) => {
    if (cosmos) {
      if (t === niveauCosmos) return;
      setNiveauCosmos(t as NiveauCosmos);
      return;
    }
    if (t === 0) {
      setNiveauCosmos(0);
      zoomTo(() => setFace("cosmos"), "out");
      return;
    }
    zoomTo(() => { setFace("cosmos"); setNiveauCosmos(t as NiveauCosmos); }, t > viewIdx ? "in" : "out");
  };
  /**
   * ── QUEL BOUTON EST ALLUMÉ : CE QUE LA SCÈNE MONTRE, PAS CE QU'ON A CLIQUÉ ──
   *
   * Le sélecteur marquait « 🌌 Cosmos » dès que `cosmos` était vrai. Or DANS le
   * Cosmos les trois dimensions sont des ÉCHELLES d'une seule et même scène —
   * c'est `viewIdx`, qui vient de `niveauAffiche`, le niveau RÉELLEMENT rapporté
   * par la scène. MESURÉ sur la page servie : après un clic sur « 🧠 Monde »
   * comme après un clic sur « 🔺 Graphe », `document.querySelector('.viewswitch
   * button.on')` répondait encore « 🌌 Cosmos ». L'utilisateur désignait une vue
   * et rien ne s'allumait.
   *
   * On allume donc l'échelle que la scène occupe vraiment, quel que soit le
   * chemin emprunté pour y arriver (bouton, molette, clavier, clic sur un monde).
   */
  const vueActive = cosmos ? viewIdx : monde ? 1 : 2;
  /**
   * ── CE QUE CHAQUE CRAN CONTIENT, LU SUR LES MESURES DE LA SCÈNE ────────────
   *
   * La scène publie déjà, pour être MESURÉE (`__etages`), ce que chaque échelle
   * porte réellement : le palier visé, la descente, les groupes du niveau, l'axe
   * qui les distingue, le mélange vers les signaux. Le curseur les LIT au lieu de
   * les recalculer : deux calculs de la même grandeur finiraient par diverger, et
   * un curseur qui annonce autre chose que ce que la scène montre est exactement
   * le défaut que ce dépôt corrige partout.
   * On ne lit que lorsque le Cosmos est à l'écran, à 400 ms — c'est un affichage,
   * pas un instrument de mesure.
   */
  const [etages, setEtages] = useState<Record<string, any> | null>(null);
  useEffect(() => {
    if (!cosmos) { setEtages(null); return; }
    const t = window.setInterval(() => {
      const e = (window as unknown as Record<string, any>).__etages;
      if (e) setEtages({ ...e });
    }, 400);
    return () => window.clearInterval(t);
  }, [cosmos]);
  /** Le nombre de nœuds/arêtes de l'espace ouvert (déjà mesuré plus haut). */
  const contenuCran = (n: number): string => {
    const o = etages?.organisation, m = etages?.monde, r = etages?.rapport;
    if (n === 0) {
      const espaces = presets.length;
      return `${espaces} espace${espaces > 1 ? "s" : ""}${matiere ? ` · ${matiere.nodes.toLocaleString("fr-FR")} nœuds ici` : ""}`;
    }
    if (n === 1) {
      const groupes = Number(o?.tous ?? 0), montres = Number(o?.montres ?? 0);
      const axe = m?.axe?.axe, nAxes = Number(m?.axe?.groupes ?? 0);
      if (groupes) return `${groupes} groupe${groupes > 1 ? "s" : ""}${montres && montres < groupes ? ` (${montres} dessinés)` : ""}${axe && nAxes ? ` · ${axe} : ${nAxes}` : ""}`;
      return matiere ? `${matiere.nodes.toLocaleString("fr-FR")} nœuds · ${matiere.edges.toLocaleString("fr-FR")} arêtes` : "composition du monde";
    }
    if (n === 2) {
      const g = Number(r?.nGraph ?? 0);
      return g ? `graphe ouvert à ${(g * 100).toFixed(0)} %` : "le graphe dans son groupe";
    }
    const tS = Number(m?.signaux?.tS ?? 0);
    return tS > 0.02 ? `mélange à ${(tS * 100).toFixed(0)} % (courbure · énergie · degré)` : "courbure · énergie · degré";
  };
  /** Les quatre crans, du plus large au plus fin — l'ordre du curseur. */
  const CRANS = [
    { mot: "Espace", ic: "🌌", quoi: "toutes les bases déployées" },
    { mot: "Monde", ic: "🧠", quoi: "l'organisation des connaissances (ses groupes)" },
    { mot: "Groupe", ic: "🔺", quoi: "le graphe réel DANS son groupe (K3/K4/∂/synapses)" },
    { mot: "Signaux", ic: "🧬", quoi: "la MÊME matière réordonnée par courbure · énergie · degré" },
  ];
  return (
    <div
      className={"shell ecran-" + ecran + (reduit ? " chrome-reduit" : "")
        + (chatOuvert ? " chat-ouvert" : "")}
      style={{
        // L'empreinte du chrome, en variables CSS : les panneaux s'y rangent et
        // la scène 3D peut s'y cadonner. Une seule source pour les deux.
        ["--chrome-top" as string]: inset.haut + "px",
        ["--chrome-bottom" as string]: inset.bas + "px",
        ["--chrome-right" as string]: inset.droite + "px",
      }}
    >
      {/* Stage : la représentation est MAÎTRESSE, plein écran */}
      <div className={"stage" + (fx ? " fx-" + fx : "")} style={{ transformOrigin: fxOrigin }}>
        <div className={"stage-veil" + (fx ? " on" : "")} />
        {/* (ancien bouton d'échelle retiré : le sélecteur des 3 vues le remplace
            — il se superposait à lui, d'où les affichages qui se marchaient dessus) */}
        {/* SÉLECTEUR DES QUATRE CRANS D'ÉCHELLE — une seule scène, quatre distances :
            espace (toutes les bases) · monde (sa composition) · groupe (son graphe)
            · signaux (la matière réordonnée par courbure · énergie · degré).
            Le quatrième existait déjà dans la scène (curseur `tS`, `posSignaux`)
            mais AUCUN contrôle n'y menait : on ne pouvait l'atteindre qu'en
            forçant la molette au-delà du fond, sans savoir qu'il existait. */}
        {/* ── L'ÉCHELLE EST UN CURSEUR, PAS QUATRE BOUTONS ────────────────────
            Quatre boutons disaient « on est à un cran », pas « où l'on est entre
            eux » : la descente est CONTINUE (mesurée : 9 paliers sans saut), et
            aucun bouton ne pouvait le montrer. Le curseur, lui, occupe la position
            RÉELLE (`vueActive`, rapportée par la scène), se tire à la souris, se
            parcourt au clavier, et — c'est la demande — il PORTE LES DONNÉES de
            chaque cran : ce que la scène y contient VRAIMENT, mesuré par elle
            (groupes du niveau, graphe ouvert, mélange vers les signaux). Un cran
            qui n'annonce pas sa matière est un cran qu'on visite à l'aveugle. */}
        <div className="echelle-curseur">
          <div className="ec-colonne">
            {CRANS.map((c, n) => (
              <button key={c.mot}
                className={"ec-cran" + (vueActive === n ? " on" : "")}
                title={`Cran ${n + 1}/4 — ${c.quoi}${etages ? ` · ici : ${contenuCran(n)}` : ""}`}
                onClick={() => goView(n as 0 | 1 | 2 | 3)}>
                <b>{c.ic} {c.mot}</b>
                <span className="ec-donnees">{contenuCran(n)}</span>
              </button>
            ))}
          </div>
          <input className="ec-rail" type="range" min={0} max={3} step={1}
            value={vueActive}
            onChange={(e) => goView(Number(e.target.value) as 0 | 1 | 2 | 3)}
            aria-label="Cran d'échelle : espace, monde, groupe, signaux"
            title="Glisser pour descendre l'échelle (le curseur occupe la position réellement mesurée)" />
          <button className={"chip" + (morph ? " on" : "")}
            title="Morph : le MÊME substrat change de forme (un seul canvas)"
            onClick={() => setMorph((v) => !v)}>⟲</button>
        </div>
        {/* Le COSMOS est désormais sa propre scène (système solaire + toile de
            l'univers) : il passe donc AVANT la branche « morph ». Sinon, comme
            `morph` est actif par défaut, la vue Cosmos restait inatteignable —
            l'utilisateur voyait l'ancienne projection « même substrat ».
            `morph` reste le morphing continu Monde ↔ Graphe. */}
        {aucuneBase ? null : cosmos ? (
          <CanvasBoundary label="Cosmos"><CosmosWeave activePath={active}
            niveau={niveauCosmos} onNiveau={rapporterNiveau}
            onOpen={ouvrirBase} /></CanvasBoundary>
        ) : morph ? (
          <CanvasBoundary label="Substrat">
            <StageContinu view={monde ? "monde" : "graphe"} activePath={active}
              onBase={(path) => { setActive(path); }} />
          </CanvasBoundary>
        ) : monde ? (
          <CanvasBoundary label="Monde">
            <StoreOverview store={active} onExit={() => setFace("graphe")}
              onDrill={(qn) => { setFace("graphe"); anchorOne(qn); }} />
          </CanvasBoundary>
        ) : (
          <CanvasBoundary label="Graphe">
            <Graph3D onDbl={(id) => pushAnchor({ centers: [id], hops: Math.max(1, hops), label: id })} />
          </CanvasBoundary>
        )}
        {/* ── LA FRATRIE SE MONTRE DANS LA FACE MONDE AUSSI ────────────────────
            Défaut mesuré (et désormais inscrit dans `scripts/fratrie-check.mjs`) :
            la porte exigeait `!monde`, et `Graph3D` n'est monté que si « ⟲ morph »
            est INACTIF — dans l'état par défaut, aucun des deux n'était vrai, donc
            `selectedNode` ne pouvait jamais être posé et la barre ne se montait
            JAMAIS. `StageContinu` publie maintenant le graphe qu'il affiche et le
            nœud de son centre ; la même porte sert donc les deux faces.
            `!cosmos` reste : le Cosmos a son propre panneau de fratrie. */}
        {!cosmos && selectedNode && montre("facettes") && <FacetBar />}
        {askMsg && <div className="ask-msg">{askMsg}</div>}

        {/* ── AUCUNE BASE LISIBLE : ON LE DIT, ET ON DONNE LA SORTIE ──────────
            Défaut mesuré le 2026-09-15, dès le premier compte ami : la liste des
            bases annonçait tous les presets du serveur, l'interface s'ancrait sur
            le premier, et toutes ses requêtes étaient refusées. L'écran restait
            vide — indistinguable d'une panne — et rien ne disait au compte qu'il
            n'avait aucun droit. Le serveur ne ment plus (`/api/stores` filtre) ;
            il faut maintenant que le VIDE s'explique.
            `compte` est le témoin d'une réponse du serveur : un écran qui n'a
            encore rien reçu n'est pas un écran sans droits. */}
        {/* ── ESPACE ACCORDÉ MAIS SANS MATIÈRE : LE MOT QUI MANQUAIT ─────────
            Le cas « aucun espace » est expliqué juste au-dessus ; celui-ci ne
            l'était nulle part. Il ne s'agit pas d'une panne : il n'y a rien à
            montrer encore. On donne le chiffre RÉEL, et les deux sorties —
            parler (le chat fonctionne sans graphe) et ajouter de la matière. */}
        {!aucuneBase && matiere && matiere.nodes < SEUIL_VIDE && (
          <div className="sans-acces">
            <b>Cet espace est encore vide{matiere.nodes > 0 ? ` — ${matiere.nodes} nœud${matiere.nodes > 1 ? "s" : ""}` : ""}.</b>
            <div style={{ marginTop: 6 }}>
              Il n'y a donc rien à explorer pour l'instant, et ce n'est pas une panne :
              {" "}<b>{activeLabel || "l'espace ouvert"}</b> ne contient pas encore de matière
              {matiere.edges ? ` (${matiere.edges} arête${matiere.edges > 1 ? "s" : ""})` : ""}.
            </div>
            <div style={{ marginTop: 8 }}>
              <b>Deux sorties :</b>
            </div>
            <div className="chips" style={{ marginTop: 4 }}>
              <button className="chip on" onClick={() => setChatOuvert(true)}
                title="Le chat répond même sans graphe : il ancre sur l'espace par défaut et votre Self">
                💬 poser une question
              </button>
              <button className="chip" onClick={() => goView(0)}
                title="Le panneau « Ajouter un monde » vit dans la vue Cosmos">
                ＋ ajouter un monde
              </button>
            </div>
          </div>
        )}
        {info && info.compte && (info.presets || []).length === 0 && (
          <div className="sans-acces">
            <b>Aucune base ne vous est accordée pour l'instant.</b>
            <div style={{ marginTop: 6 }}>
              Connecté comme <b>{info.compte}</b>
              {info.espaces?.length
                ? <> — espace(s) accordé(s) : <b>{info.espaces.join(", ")}</b></>
                : <> — <b>aucun espace</b> ne vous est accordé</>}.
            </div>
            {info.refuses?.length ? (
              <div style={{ marginTop: 6, opacity: 0.8 }}>
                {info.refuses.length} base(s) existent mais ne vous sont pas ouvertes
                {" "}({info.refuses.slice(0, 5).join(", ")}
                {info.refuses.length > 5 ? "…" : ""}) : demandez à l'administrateur
                de vous accorder leur espace.
              </div>
            ) : null}
            <div style={{ marginTop: 8 }}>
              <b>Vous pouvez tout de suite ajouter vos mondes :</b> ouvrez la fenêtre
              <b> « Ajouter un monde »</b> (à droite) et collez l'adresse d'un dépôt
              GitHub — il sera cloné, indexé, enrichi, et il vous appartiendra.
            </div>
          </div>
        )}
      </div>

      {/* Barre de commandes posée sur le canvas — soumise à la règle d'affichage */}
      {montre("commandes") && <CommandBar
        value={cmd}
        onChange={setCmd}
        onSubmit={ask}
        onAnchor={anchorOne}
        presets={presets}
        activeLabel={activeLabel}
        store={active}
        onGoPreset={goPreset}
        anchors={history}
        onAnchorNav={goAnchor}
      />}

      {/* ── LA CONVERSATION, AU-DESSUS DE LA VUE ─────────────────────────
          Elle n'est pas un enfant du canvas : elle le recouvre quand elle est
          ouverte, et le bouton flottant est là dès qu'elle est fermée — sans
          dépendre d'un panneau (sur téléphone les panneaux sont repliés).
          ── LA PORTE NE SE FERME PAS QUAND RIEN N'EST OUVERT ───────────────
          Elle était masquée dès qu'il n'y avait AUCUNE base (`aucuneBase ? null`),
          c'est-à-dire exactement à la PREMIÈRE CONNEXION — le seul moment où l'on
          a le plus besoin de parler. Le panneau, lui, restait monté « pour
          expliquer ce qui manque » : fermé ET sans bouton, il était donc
          inaccessible. Un compte neuf voyait des explications sans pouvoir poser
          une question. Le bouton reste : même porte dans toutes les vues, et dans
          le cas vide aussi. */}
      {!chatOuvert ? (
        <button className="chat-flottant" onClick={() => setChatOuvert(true)}
          aria-label="Ouvrir la conversation" title="Ouvrir la conversation (plein écran)">
          💬
        </button>
      ) : null}
      {/* Timeline d'une session : ouverture plein écran, id de session entré à la main
          (l'explorateur de sessions viendra ensuite). */}
      {!tlOuvert ? (
        <button className="chat-flottant tl-flottant" onClick={() => setTlOuvert(true)}
          aria-label="Timeline d'une session" title="Timeline d'une session (prompts, appels, tokens)">
          🕐
        </button>
      ) : null}
      {tlOuvert && (
        <div className="tl-voile">
          <div className="tl-entete">
            <input
              className="tl-champ"
              placeholder="session-… (id de session runtime)"
              value={tlSession}
              onChange={(e) => setTlSession(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            />
            <button className="chip" onClick={() => setTlOuvert(false)}>fermer</button>
          </div>
          {tlSession.trim() ? (
            <TimelineView session={tlSession.trim()} store={runtimeLike(activeLabel || "") ? active : undefined} />
          ) : (
            <div className="tl-vide">Colle un id de session pour voir son fil d'exécution.</div>
          )}
        </div>
      )}
      {/* Monté même sans base : la conversation explique alors ce qui manque
          (aucun espace accordé) au lieu de rester inaccessible. */}
      <ChatPleinEcran store={storeChat} ouvert={chatOuvert}
        onFermer={() => setChatOuvert(false)} />

      {/* LES GENS — comptes et résonance, dans un seul panneau.
          Monté à la racine, au-dessus de la vue : il ne se pose donc pas SOUS
          elle, et il se retire pendant la conversation (règle CSS `.chat-ouvert`)
          au lieu de flotter sur la saisie — le défaut rapporté. */}
      <PanneauGens />
      {/* LE MONDE — ajouter un dépôt GitHub, et interroger l'engine une fois.
          Monté à la racine comme le chat et les gens : hors du canvas, donc ni
          la charge GPU de la scène ni son échec ne l'emportent — or c'est le
          geste par lequel un compte neuf obtient sa première base. */}
      <PanneauMonde />
      {/* ── MA FLOTTE : MES MACHINES, ET L'ACTION SUR ELLES ──────────────────
          Monté à la racine, comme `AdminComptes`, et pour la même raison : la
          flotte n'est pas une propriété du graphe REGARDÉ. Un panneau du canvas
          (barre de commandes, inspecteur) disparaît en chrome réduit et change
          avec la face — une machine injoignable, elle, doit rester joignable
          depuis n'importe quel écran. Le panneau n'obéit donc à aucune règle de
          réduction ; il s'ouvre par le bouton 🖥 (bas gauche) ou par l'URL
          `?flotte=1`. Voir `graph/PanneauFlotte.tsx` et `graph/flotte.ts`. */}
      <PanneauFlotte />
      {/* Le GESTE de réduction : masquer ce qui n'est pas la navigation. */}
      <button className={"chip reduire" + (reduit ? " on" : "")} aria-label="Réduire le chrome"
        aria-pressed={reduit} title="Réduire : ne garder que l'essentiel"
        onClick={() => setReduit((v) => !v)}>{reduit ? "⤢ afficher" : "⤡ réduire"}</button>

      {/* Point de contrôle : inspecteur */}
      {montre("reglages") && <button className={"lab-btn" + (ctlOpen ? " on" : "")} aria-label="Réglages"
        aria-expanded={ctlOpen} title="Réglages (⌘K)"
        onClick={() => setCtlOpen((o) => !o)}>
        <span className="lab-ico">⚙</span>
        <span className="lab-txt">Réglages</span>
      </button>}

      {/* ── LA PROJECTION NE DOIT JAMAIS ÊTRE UN CUL-DE-SAC (2026-09-13) ──────
          Défauts mesurés : (a) la projection n'existait qu'à l'intérieur de
          l'`Inspector`, lui-même monté sous `montre("reglages")` — donc sous le
          geste « ⤡ réduire » elle devenait inatteignable ; (b) après un clic sur
          sa croix, `projectionVisibleAtom` restait faux et AUCUN contrôle ne le
          relisait. Ce bouton-ci n'est soumis à AUCUNE règle d'affichage : il
          rouvre l'inspecteur ET la projection, y compris en chrome réduit. */}
      <button className={"chip proj-open" + (ctlOpen && projVisible ? " on" : "")}
        aria-label="Projection" aria-pressed={ctlOpen && projVisible}
        title="Montrer la projection multi-axes du dernier résultat"
        onClick={() => { setCtlOpen(true); setProjVisible(true); }}>⌗ projection</button>

      {/* L'inspecteur est monté dès que l'utilisateur l'a DEMANDÉ (`ctlOpen`),
          même en chrome réduit : une décision explicite prime sur la priorité
          d'affichage (voir `chromeAtom` dans app/store.ts). Le bouton ⚙, lui,
          reste soumis à la règle — c'est lui qui est du « confort ». */}
      {ctlOpen && <Inspector onClose={() => setCtlOpen(false)} />}

      {/* Statut */}
      {montre("statut") && (
        <div className="stats">
          {stats}
          <button className="stats-reload" title="Recharger" onClick={() => recharger()}>↻</button>
        </div>
      )}
    </div>
  );
}
