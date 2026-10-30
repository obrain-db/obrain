/**
 * ── LE PONT D'ÉTAT : LE NAVIGATEUR ET LE SERVEUR, UN SEUL DOCUMENT ──────────
 *
 * Ce module fait ce qu'aucun autre ne faisait : il PORTE l'état d'interface
 * jusqu'au serveur, et le ramène. Jusqu'ici, la face, l'échelle, les panneaux
 * et le sujet ne vivaient qu'en `localStorage` et dans l'URL — donc nulle part
 * ailleurs que sur ce poste, et jamais partagés.
 *
 * ── DEUX RÈGLES, ET ELLES NE SONT PAS NÉGOCIABLES ──────────────────────────
 *
 * 1. **On n'écrit QUE les atoms d'AUTORITÉ.** `faceAtom`, `niveauDemandeAtom`,
 *    `inspectorOpenAtom`, `chromeAtom`, `subjectAtom` sont des sources ;
 *    `estCosmosAtom`, `estMondeAtom`, `montreAtom` en sont DÉRIVÉS. Écrire dans
 *    un dérivé recréerait exactement la seconde autorité que toute cette refonte
 *    a supprimée — un état qui peut diverger de la vérité qu'il prétend refléter.
 *
 * 2. **Le dashboard fonctionne SANS le pont.** Si le serveur d'état est absent
 *    (aucun `this-rs` en face), on ne casse rien et on ne ment pas : on le
 *    journalise UNE fois, et l'interface continue avec son état local. Un pont
 *    obligatoire transformerait une fonctionnalité en panne.
 *
 * ── LA PORTÉE ─────────────────────────────────────────────────────────────
 * Le document est écrit par PORTÉE (`/ui/state/{portee}`), et la garde du
 * serveur compare la portée du chemin à l'appelant : c'est une ressource
 * personnelle. On utilise donc l'identité de session quand elle existe, sinon
 * la base consultée — jamais une constante partagée, qui ferait écrire deux
 * personnes dans le même document.
 */
import { getDefaultStore } from "jotai";
import {
  activeStoreAtom,
  chromeAtom,
  faceAtom,
  inspectorOpenAtom,
  axeNavigationAtom,
  niveauDemandeAtom,
  subjectAtom,
  type AxeNavigation,
  type Face,
  type NiveauCosmos,
} from "./store";

/** Base des routes d'état. Surchargeable au déploiement (voir `this-rs`). */
const BASE = (import.meta.env?.VITE_UI_STATE_BASE as string | undefined) ?? "/ui/state";

/**
 * ── LES QUATRE CRANS, NOMMÉS UNE FOIS (2026-09-15) ───────────────────────────
 *
 * Le pont portait l'échelle en clair des deux côtés — `… : "groupe"` à l'écriture,
 * `{ cosmos: 0, monde: 1, groupe: 2, noeud: 2 }` à la lecture — et les deux
 * tables divergeaient déjà : le cran 3 (que le document ne nommait pas) se serait
 * écrit « groupe » et relu « groupe », donc un document partagé aurait SILENCIEUSEMENT
 * ramené le quatrième cran au troisième. Les deux sens passent donc par une seule
 * table, et le cran ajouté y est nommé.
 */
const ECHELLE_NOM: Record<number, string> = { 0: "cosmos", 1: "monde", 2: "groupe", 3: "signaux" };
const ECHELLE_NIVEAU: Record<string, NiveauCosmos> = {
  cosmos: 0, monde: 1, groupe: 2, signaux: 3,
  // « noeud » : nom hérité du tronc, qui désignait le troisième cran sous un
  // autre mot. Il continue de se lire — un document ancien ne doit pas perdre
  // son échelle — mais il ne s'écrit plus.
  noeud: 2,
};

/** Délai d'agrégation : un geste produit plusieurs écritures, on n'en fait qu'une. */
const DELAI_MS = 400;

/** Ce qui voyage. Les noms suivent le document côté serveur (`UiState`). */
/** Les axes, nommés UNE fois — le document partagé les porte par leur nom. */
const AXE_NOM: Record<string, AxeNavigation> = {
  descendance: "module",
  module: "module",
  parent: "parent",
  extension: "extension",
  communaute: "communauté",
  "communauté": "communauté",
  couche: "couche",
  strate: "couche",
  contour: "∂",
  "∂": "∂",
};

interface DocumentUi {
  schema?: string;
  vue?: string;
  echelle?: string;
  /**
   * L'AXE DE NAVIGATION — la relation selon laquelle on descend.
   *
   * Il voyage à côté de l'échelle parce que les deux forment la même décision :
   * « où je suis » et « selon quoi je découpe ». Ne porter que l'échelle laissait
   * l'axe derrière, donc un document partagé restituait la position sans la
   * relation — et deux personnes regardant la même échelle voyaient deux
   * découpages différents sans que rien ne le dise.
   */
  axe?: string;
  panneaux?: Record<string, boolean>;
  sujet?: { store?: string; qn?: string; label?: string; via?: string };
}

let signale = false;
let minuteur: number | null = null;
/**
 * ── LE SERVEUR D'ÉTAT EST-IL LÀ ? ON LE DEMANDE UNE FOIS ───────────────────
 *
 * Défaut constaté dans la console du navigateur (2026-09-12) : le pont émettait
 * un `GET /ui/state/local` qui répondait **404**, puis un `PUT` qui répondait
 * **405**, ET RECOMMENÇAIT à chaque changement d'état. C'est exact — le
 * dashboard est servi par `obrain viz serve`, qui n'expose pas `/ui/state` :
 * seule l'implémentation `this-rs` l'expose. Mais un client qui insiste sur une
 * route inexistante salit la console, consomme le réseau et laisse croire à une
 * panne, alors que l'application fonctionne parfaitement en local.
 *
 * On sonde donc UNE fois. Sans réponse utilisable : on se tait, on cesse
 * d'envoyer, et on ne réessaie qu'après un long délai (au cas où `this-rs`
 * démarre après le navigateur). `indisponible` est lu par `planifier` : aucune
 * écriture n'est tentée tant qu'on n'a pas de serveur.
 */
let indisponible = false;
let derniereSonde = 0;
const RESONDE_MS = 120_000;

/**
 * ── LA SONDE, ET POURQUOI TOUT ÉCRIVAIN DOIT L'ATTENDRE ────────────────────
 *
 * Défaut constaté dans la console du navigateur : `GET /ui/state/…` → **404** et
 * `PUT /ui/state/…` → **405** apparaissaient ENSEMBLE. Ce n'était pas un réglage
 * à ajuster mais une COURSE : la sonde est asynchrone, et un changement d'état
 * survenant dans les premières centaines de millisecondes (résolution du store,
 * pose du sujet, ouverture d'un panneau) déclenchait le `PUT` du debounce AVANT
 * que l'échec du `GET` ait été enregistré. Marquer « absent » plus tôt ne suffit
 * pas — il faut que PERSONNE ne parte avant que la réponse soit connue.
 *
 * `sonde` est donc une promesse unique : le premier appelant la crée (une seule
 * requête, qui applique aussi le document reçu), tous les autres l'attendent. Un
 * écrivain qui arrive pendant la sonde attend son verdict ; après, il regarde
 * `indisponible` et se tait. C'est la même leçon que partout ailleurs dans ce
 * travail : une autorité unique, et personne qui court devant elle.
 */
let sonde: Promise<boolean> | null = null;

/** Attend le verdict de la sonde (en la lançant si c'est le premier appel). */
export function serveurPret(): Promise<boolean> {
  if (!sonde) sonde = sonder();
  return sonde;
}

/** Une seule requête de lecture : elle applique le document s'il existe. */
async function sonder(): Promise<boolean> {
  try {
    const r = await fetch(`${BASE}/${encodeURIComponent(portee())}`, { method: "GET" });
    if (!r.ok) {
      marquerAbsent(`HTTP ${r.status}`);
      return false;
    }
    const j = (await r.json()) as { ui?: DocumentUi };
    if (j && typeof j === "object" && j.ui) appliquer(j.ui);
    return true;
  } catch (e) {
    marquerAbsent(String((e as Error)?.message ?? e));
    return false;
  }
}

/** Sonde le serveur d'état. Rend vrai s'il répond quelque chose d'utilisable. */
async function serveurDisponible(): Promise<boolean> {
  const maintenant = Date.now();
  if (!indisponible) return true;
  if (maintenant - derniereSonde < RESONDE_MS) return false;
  derniereSonde = maintenant;
  try {
    const r = await fetch(`${BASE}/${encodeURIComponent(portee())}`, { method: "GET" });
    if (r.ok) {
      indisponible = false;
      return true;
    }
  } catch {
    /* toujours absent : on reste discret */
  }
  return false;
}

/** Marque le serveur absent — une seule fois, sans bruit. */
function marquerAbsent(raison: string): void {
  indisponible = true;
  // Armer le délai ICI : sans cela `derniereSonde` reste à 0, `Date.now() - 0`
  // dépasse le seuil et autorise une sonde immédiate — mesuré : un second GET
  // partait aussitôt, avant même que l'utilisateur ait touché à quoi que ce soit.
  derniereSonde = Date.now();
  if (!signale) {
    signale = true;
    console.info(
      `[ui-state] aucun serveur d'état d'interface (${raison}) : l'interface ` +
      "fonctionne en local. `this-rs` expose /ui/state/{portee} quand il sert " +
      "le dashboard ; `VITE_UI_STATE_BASE` permet de pointer ailleurs.",
    );
  }
}

function portee(): string {
  const params = new URLSearchParams(window.location.search);
  // `portee` explicite d'abord (partage d'une vue), sinon la base consultée.
  return params.get("portee") || getDefaultStore().get(activeStoreAtom) || "local";
}

function lireLocal(): DocumentUi {
  const store = getDefaultStore();
  const etat = store.get(niveauDemandeAtom);
  return {
    schema: "this-rs/ui-state/1",
    vue: store.get(faceAtom),
    echelle: ECHELLE_NOM[etat] ?? "cosmos",
    axe: store.get(axeNavigationAtom),
    panneaux: {
      ...store.get(chromeAtom),
      reglages: store.get(inspectorOpenAtom),
    },
    sujet: getDefaultStore().get(subjectAtom),
  };
}

/**
 * Applique un document reçu — en n'écrivant que les autorités.
 * On ne touche QUE ce que le document porte : un champ absent reste tel quel,
 * plutôt que d'être écrasé par un défaut que l'appelant n'a pas demandé.
 */
function appliquer(doc: DocumentUi): void {
  const store = getDefaultStore();
  if (doc.vue === "cosmos" || doc.vue === "monde" || doc.vue === "graphe") {
    store.set(faceAtom, doc.vue as Face);
  }
  const echelle: Record<string, NiveauCosmos> = ECHELLE_NIVEAU;
  if (doc.echelle && doc.echelle in echelle) {
    store.set(niveauDemandeAtom, echelle[doc.echelle]);
  }
  if (doc.axe) {
    const axe = AXE_NOM[doc.axe.trim().toLowerCase()];
    // Un axe INCONNU ne s'applique pas : on ne devine pas ce que l'appelant a
    // voulu dire, et un axe muet vaudrait mieux qu'un axe faux.
    if (axe) store.set(axeNavigationAtom, axe);
  }
  if (doc.panneaux) {
    const { reglages, ...reste } = doc.panneaux;
    store.set(chromeAtom, reste);
    if (typeof reglages === "boolean") store.set(inspectorOpenAtom, reglages);
  }
  const s = doc.sujet;
  if (s?.qn || s?.store) {
    store.set(subjectAtom, {
      store: s.store ?? "", qn: s.qn ?? "", label: s.label ?? s.qn ?? "", via: "",
    });
  }
}

async function pousser(): Promise<void> {
  // ON ATTEND LE VERDICT : c'est ce qui ferme la course entre le GET et le PUT.
  if (!(await serveurPret())) return;
  if (indisponible && !(await serveurDisponible())) return;
  try {
    const r = await fetch(`${BASE}/${encodeURIComponent(portee())}`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(lireLocal()),
    });
    if (!r.ok) marquerAbsent(`PUT HTTP ${r.status}`);
  } catch (e) {
    marquerAbsent(String((e as Error)?.message ?? e));
  }
}

function planifier(): void {
  if (minuteur !== null) window.clearTimeout(minuteur);
  minuteur = window.setTimeout(() => {
    minuteur = null;
    void pousser();
  }, DELAI_MS);
}

/** Démarre le pont : charge l'état distant, puis suit les autorités. */
export function demarrerPontEtat(): () => void {
  const store = getDefaultStore();
  const arret = [
    store.sub(faceAtom, planifier),
    store.sub(niveauDemandeAtom, planifier),
    store.sub(inspectorOpenAtom, planifier),
    store.sub(chromeAtom, planifier),
    store.sub(subjectAtom, planifier),
    store.sub(axeNavigationAtom, planifier),
  ];

  // Lecture initiale : la MÊME sonde que celle qu'attendent les écrivains, donc
  // une seule requête au total — et le serveur fait autorité quand il répond.
  void serveurPret();

  return () => {
    arret.forEach((s) => s());
    if (minuteur !== null) window.clearTimeout(minuteur);
  };
}
