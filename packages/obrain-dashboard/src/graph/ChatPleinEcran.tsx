import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useAtom } from "jotai";
import { subjectAtom } from "../app/store";
import {
  type Etat, cleEtat, cleAncienne, courante, etatNeuf, migrer, majCourante,
  ajouter, choisir, renommer, ranger, epingler, supprimer, trier, dossiers,
  aPersister, nouvelId, titreAuto, titreVide,
} from "./conversations";
import { fetchEntretien, isAbort, type Entretien } from "../lib/api";
import { MarkdownRiche } from "./RenduRiche";
import { ATTENTE_RACCORD, raccorder } from "./raccord";

/**
 * CHAT PLEIN ÉCRAN — la conversation comme un lieu, pas comme un encart.
 *
 * Ce que ce composant apporte et que l'encart ne pouvait pas :
 *   - PLEIN ÉCRAN sur mobile (un vrai espace de conversation, refermable) ;
 *   - le flux en TEMPS RÉEL : le contexte obrain s'annonce d'abord, puis le
 *     *think*, puis la réponse — on voit le travail se faire au lieu d'attendre
 *     une réponse en bloc (un modèle de raisonnement réfléchit 60 s : la fenêtre
 *     restait muette tout ce temps) ;
 *   - une CONVERSATION : l'historique est transmis au serveur, donc le modèle
 *     garde le fil ;
 *   - un ancrage sur le GRAPHE GLOBAL quand aucun sujet n'est visé — on peut
 *     ouvrir le chat et interroger l'univers entier.
 *
 * Le transport est un `EventSource` (SSE) : le serveur pousse, le client
 * affiche. Rien n'est simulé — chaque morceau affiché vient du réseau.
 */

interface Tour {
  role: "user" | "assistant";
  texte: string;
  think?: string;
  contexte?: {
    noeuds: number;
    relations: number;
    apercu: string;
    ms: number;
    /** Chemin du store RÉELLEMENT ouvert — rapporté par le serveur. */
    store?: string;
    /** Libellé du monde lu, quand le sujet en désigne un. */
    monde?: string | null;
    total_noeuds?: number;
    total_aretes?: number;
    /** Répartition par label : la densification disponible, chiffrée. */
    par_label?: { label: string; total: number; lus: number }[];
    /** Le Self propriétaire de cette conversation. */
    self_qn?: string;
    /** Fenêtre de jetons réellement servie par le modèle, et sa provenance. */
    fenetre?: number;
    fenetre_source?: string;
    /** Les nœuds les plus connectés — le « cœur » du graphe, lu par registre. */
    hubs?: string[];
    /** Temps du calcul de ce classement (mesuré par l'entretien). */
    hubs_ms?: number;
  };
  /** Les appels d'outils RÉELLEMENT exécutés contre la base, dans l'ordre. */
  outils?: OutilAppel[];
  /** Ce que la conversation a STRUCTURÉ durablement (écriture journalisée). */
  memoires?: Memoire[];
  ms_llm?: number;
  /** `stop` = fin naturelle · `length` = plafond de tokens · `flux_coupe` = le
   *  flux du modèle s'est arrêté sans dire qu'il avait fini. */
  raison?: string | null;
  /** La question qui a produit ce tour — une reprise la repose telle quelle. */
  question?: string;
  /** Combien de fois ce tour a été REPRIS après une coupure du flux. */
  reprises?: number;
  /** Une reprise automatique est en cours. */
  reprise?: boolean;
  /** L'échec peut être rejoué à la main (bouton « reprendre »). */
  erreurReprenable?: boolean;
  /** La reprise a rendu une réponse ENTIÈRE (le modèle a régénéré, il n'a pas
   *  continué) : c'est dit, parce que le texte affiché a alors été remplacé. */
  repriseComplete?: boolean;
  erreur?: string;
  encours?: boolean;
}

/**
 * ── L'HISTORIQUE SURVIT AU RECHARGEMENT ─────────────────────────────────────
 *
 * Avant, les tours vivaient dans un `useState` : recharger la page — ou rouvrir
 * le chat — effaçait la conversation, alors que le serveur, lui, la reçoit et
 * s'en sert. L'utilisateur perdait donc SES messages, et le modèle repartait
 * sans ce qu'il venait de dire.
 *
 * Conservé par NAVIGATEUR (`localStorage`), **par store** : deux bases servies
 * sont deux conversations différentes, et les mélanger ferait lire à l'un le
 * contexte de l'autre.
 *
 * On ne restaure jamais un tour « en cours » : au rechargement le flux est
 * mort, et un tour marqué en cours ferait tourner une attente éternelle.
 */
/**
 * ── L'HISTORIQUE EST CELUI D'UN SELF, PAS D'UN NAVIGATEUR ──────────────────
 *
 * Défaut mesuré le 2026-09-16 : la clé ne portait que la BASE
 * (`obrain.chat.v1.<store>`). Or plusieurs personnes peuvent ouvrir le même
 * navigateur, et chacune a SON self — le serveur le dit, et le panneau l'affiche
 * (« self : … »). Sans le self dans la clé, la conversation d'un compte restait
 * à l'écran du suivant : exactement le mélange que le reste du chantier combat
 * (mémoire d'un compte écrite chez un autre).
 *
 * Le self arrive APRÈS le montage (le serveur le décide — il ne se choisit plus
 * par l'appelant). On garde donc le repli « par base » pour la fenêtre qui ne
 * sait pas encore qui elle est, puis l'emplacement se fixe dès que le self est
 * connu : c'est `migrerPortee`, plus bas.
 *
 * ── CE QUE CETTE FONCTION EST DEVENUE AVEC LE CAHIER (fusion) ──────────────
 * Depuis que l'état est un CAHIER de conversations (`conversations.ts`), le
 * contenu rangé vit sous `cleEtat(portée)` — même préfixe `obrain.chat.v2.`, mais
 * une seule clé porte alors PLUSIEURS conversations. `cleHistorique` n'est donc
 * plus la clé de rangement : elle ne sert plus qu'à LIRE et DÉMÉNAGER l'ancienne
 * forme (un fil unique), ce que fait `migrerPortee` pour les deux emplacements.
 * La garder sous ce nom, plutôt que la supprimer, est délibéré : c'est ce qui
 * permet à un utilisateur qui avait un historique au format précédent de le
 * retrouver — un format qu'on abandonne sans le lire est un format qu'on perd.
 */
const cleHistorique = (scope: string) => `obrain.chat.v2.${scope}`;
/**
 * ── LA PORTÉE D'UN CAHIER : DU PLUS PRÉCIS AU PLUS PROVISOIRE (fusion) ─────
 *
 * `self:<qn>` — le self RÉSOLU (annoncé par `contexte`, en fin de réflexion) ;
 * `compte:<nom>` — le compte (annoncé par `identite`, dès l'ouverture) ;
 * `base:<store>` — provisoire, tant que le serveur n'a rien dit.
 *
 * Chaque palier isole mieux que le précédent, et AUCUN ne laisse deux personnes
 * se partager l'écran : c'est la propriété qui compte ici, pas la finesse.
 */
const cleSansSelf = (store: string) => `base:${store}`;
/**
 * LA PORTÉE COURANTE — la fonction UNIQUE qui décide sous quelle clé le cahier
 * de conversations (`conversations.ts`) est lu et écrit.
 *
 * Les deux lignées posaient chacune la moitié de cette clé :
 *   · le côté canonique a établi QU'ELLE PORTE (le self, puis le compte, puis
 *     la base provisoire) — sans quoi la conversation d'un compte restait à
 *     l'écran du suivant, sur le même navigateur ;
 *   · le côté multi-écrivain a établi CE QU'ELLE CONTIENT (un cahier de
 *     conversations rangées, sous `cleEtat` = `obrain.chat.v2.<portée>`).
 * Les deux portent le MÊME préfixe `obrain.chat.v2.` : la portée devient donc
 * simplement l'argument du cahier. Un self n'a pas UNE conversation mais
 * PLUSIEURS, et deux personnes sur un navigateur n'ont pas le même cahier.
 */
const porteeDe = (selfQn: string | null, compte: string | null, store: string) =>
  selfQn || (compte ? `compte:${compte}` : cleSansSelf(store));

/** La portée d'un emplacement de stockage quelconque. */
const porteeDeCle = (cle: string) => cle.slice("obrain.chat.v2.".length);

/** Le tour du chat, tel que le cahier le manipule (mêmes champs, indexation en plus). */
export type TourC = Tour & Record<string, unknown>;

export function chargerHistorique(store: string): Tour[] {
  try {
    const brut = localStorage.getItem(cleHistorique(store));
    if (!brut) return [];
    const gardes = JSON.parse(brut) as Tour[];
    if (!Array.isArray(gardes)) return [];
    return gardes.map((t) => ({ ...t, encours: false, reprise: false, erreurReprenable: false }));
  } catch {
    return [];
  }
}

export function oublierHistorique(store: string) {
  try { localStorage.removeItem(cleHistorique(store)); } catch { /* stockage refusé */ }
}

/**
 * L'état rangé d'un SELF : le format v2 s'il existe, sinon la MIGRATION de
 * l'ancien fil unique, sinon un état neuf. Dans tous les cas on ne restaure pas
 * un tour « en cours » — au rechargement le flux est mort, et un tour marqué en
 * cours ferait tourner une attente éternelle.
 *
 * `portee` est la PORTÉE du cahier (voir `porteeDe`) : `cleEtat` en fait la clé
 * définitive, et la portée provisoire (`base:<store>`) est donc elle aussi
 * migrée — un échange entamé avant que le serveur ait annoncé l'identité ne
 * reste pas orphelin.
 */
function chargerEtat(portee: string): Etat<TourC> {
  try {
    const brut = localStorage.getItem(cleEtat(portee));
    if (brut) {
      const e = JSON.parse(brut) as Etat<TourC>;
      if (e && Array.isArray(e.liste) && e.liste.length) {
        return {
          ...e,
          liste: e.liste.map((c) => ({
            ...c,
            tours: (c.tours || []).map((t) => ({ ...t, encours: false, reprise: false, erreurReprenable: false })),
          })),
        };
      }
    }
    const migre = migrer<TourC>(JSON.parse(localStorage.getItem(cleAncienne(portee)) || "null"));
    if (migre) return migre;
  } catch { /* stockage refusé ou contenu illisible : on repart d'un état neuf */ }
  return etatNeuf<TourC>();
}

/**
 * ── LE CAHIER DÉMÉNAGE AVEC SON PROPRIÉTAIRE (fusion des deux lignées) ─────
 *
 * Deux défauts mesurés, corrigés chacun par une lignée, et qui se composent :
 *
 *  1. « l'état ne se rangeait que par BASE » (lignée canonique) : la
 *     conversation d'un compte restait à l'écran du suivant. D'où la portée.
 *  2. « un cahier chargé ailleurs écrasait l'espace courant » (lignée
 *     multi-écrivain) : le self et le compte n'arrivent qu'APRÈS le montage
 *     (`identite` puis `contexte`), donc le cahier de la portée provisoire
 *     vivait sous `base:<store>` et y restait. On DÉMÉNAGE donc ce qui y a été
 *     écrit dès que la portée définitive est connue — et on retire la clé
 *     provisoire, sinon elle resterait un doublon lisible par le compte suivant.
 *
 * On migre les DEUX formats d'emplacement : le cahier (`cleEtat`) et l'ancien
 * fil unique (`cleHistorique`, format v1 de la lignée canonique) — la clé
 * provisoire peut porter l'un ou l'autre selon ce qui a été écrit en premier.
 * Non destructif : une portée définitive déjà remplie n'est jamais écrasée.
 */
function migrerPortee(portee: string, definitive: string) {
  if (!definitive || definitive === portee) return;
  try {
    for (const cle of [cleEtat, cleHistorique]) {
      const reste = localStorage.getItem(cle(portee));
      if (reste && !localStorage.getItem(cle(definitive))) {
        localStorage.setItem(cle(definitive), reste);
      }
      if (reste) localStorage.removeItem(cle(portee));
    }
  } catch { /* stockage refusé : la conversation ne doit pas échouer pour ça */ }
}

/**
 * LA RÉPONSE, AVEC LES APPELS D'OUTILS **À LEUR PLACE**.
 *
 * Un appel d'outil n'est pas un journal à part : c'est ce que le modèle a FAIT
 * pour répondre, et il a eu lieu à un endroit précis. `segmentsDeReponse` découpe
 * le texte à ces endroits — sans jamais couper un bloc Markdown ni un bloc de
 * code — et chaque morceau repasse par `MarkdownRiche`, donc la mémoire de blocs
 * (une formule déjà affichée ne repasse pas dans KaTeX) reste valable.
 *
 * Seul le DERNIER morceau est « actif » : c'est le seul qui grandit encore
 * pendant le flux.
 */
function ReponseAvecOutils({ t }: { t: Tour }) {
  const morceaux = segmentsDeReponse(t.texte || "", t.outils);
  const dernierTexte = morceaux.reduce((n, s, i) => ("texte" in s ? i : n), -1);
  return (
    <>
      {morceaux.map((seg, k) =>
        "outil" in seg ? (
          <div key={`o${k}`} className={seg.outil.erreur ? "chat-outil err" : "chat-outil"}>
            <b>{seg.outil.nom}</b>
            <code title={seg.outil.arguments}>{seg.outil.arguments || "()"}</code>
            {seg.outil.erreur ? (
              <span className="chat-outil-err"> → {seg.outil.erreur}</span>
            ) : (
              <span className="chat-outil-ok">
                {" "}→ {seg.outil.lignes} ligne{seg.outil.lignes > 1 ? "s" : ""} · {seg.outil.ms} ms
                {seg.outil.tronque ? (
                  <span className="chat-outil-coupe" title="contenu coupé au plafond de caractères — le modèle a reçu ce qui précède, pas la suite">
                    {" "}· coupé
                  </span>
                ) : null}
              </span>
            )}
          </div>
        ) : (
          <MarkdownRiche key={`t${k}`} texte={seg.texte} actif={t.encours && k === dernierTexte} />
        ),
      )}
    </>
  );
}

import { segmentsDeReponse, type OutilAppel } from "./segments";

export type { OutilAppel };


/**
 * Ce que la conversation a structuré : une écriture JOURNALISÉE.
 *
 * On ne dit pas « enregistré » mais « mémorisé, en attente d'application » : le
 * substrat impose l'invariant mono-écrivain, le serveur tient ses bases en
 * lecture, et l'application se fait par une passe dédiée. Annoncer une écriture
 * faite alors qu'elle est en attente serait exactement le mensonge qu'on corrige
 * partout ailleurs dans ce chat.
 */
export interface Memoire {
  genre: string;
  reference: string;
  self_qn: string;
}

/**
 * COMBIEN DE FOIS UNE RÉPONSE COUPÉE EST REPRISE, ET À QUEL RYTHME.
 *
 * Mesuré : le service `viz serve` redémarre en ~2 s (systemd, `RestartSec=2`),
 * donc une coupure due à un déploiement se répare presque sans attente. Les
 * délais s'allongent ensuite — un réseau tombé ne revient pas plus vite parce
 * qu'on insiste, et une reprise qui boucle contre un service absent serait une
 * panne de plus. Au-delà du plafond, l'échec est DIT et rejouable à la main.
 */
const MAX_REPRISES = 4;
const DELAIS_REPRISE_MS = [1200, 2000, 3000, 5000];

/** Nom court d'un chemin de store : le fichier, sans le répertoire. */
const nomStore = (chemin?: string) =>
  chemin ? chemin.split("/").filter(Boolean).pop() || chemin : "";

/**
 * LA PART LUE, EN CLAIR.
 *
 * Défaut mesuré (2026-09-11) : le chat annonçait « 24 nœuds · 12 relations »
 * pour un monde qui en contient 4 685 — sans dire que c'était 0,5 %. Et ces
 * 24 nœuds étaient 22 titres de `.github/ISSUE_TEMPLATE/`, aucune fonction.
 *
 * Un plafond silencieux fait croire que la donnée n'existe pas : c'est pire
 * qu'une donnée manquante. On affiche donc le numérateur ET le dénominateur.
 */
function partLue(c: NonNullable<Tour["contexte"]>): string {
  if (!c.total_noeuds || c.total_noeuds <= 0) return "";
  const pct = (100 * c.noeuds) / c.total_noeuds;
  return `${c.noeuds} / ${c.total_noeuds} nœuds (${pct < 0.1 ? pct.toFixed(2) : pct.toFixed(1)} %)`;
}

/**
 * ── POURQUOI LE FLUX EST MORT : ON LE VÉRIFIE, ON NE LE DEVINE PAS ────────
 *
 * `EventSource` ne dit RIEN de la cause : un serveur absent, un store
 * introuvable et une question refusée produisent tous le même `onerror`. La vue
 * annonçait donc « le serveur a été arrêté ou relancé » — une cause PLAUSIBLE
 * mais non vérifiée, qui accusait le serveur quand il avait en fait REFUSÉ la
 * question (store inexistant). On demande donc au serveur s'il répond : c'est un
 * fait, et il discrimine les deux causes réelles.
 */
async function diagnostiquerCoupure(): Promise<string> {
  try {
    const r = await fetch("/api/stores");
    if (r.ok) {
      return "le serveur répond, mais la question s'est fermée avant le premier mot : " +
        "elle a été refusée (store introuvable, ou paramètre incomplet) — le journal du serveur porte la cause exacte";
    }
    return `le serveur a répondu ${r.status} à la question, avant tout contexte : requête refusée`;
  } catch {
    return "aucun serveur ne répond : `obrain viz serve` n'est pas en marche, la question n'a pas atteint l'engine";
  }
}

/** Les niveaux de raisonnement, dans l'ordre du cycle du bouton. `moyenne`
 *  d'abord : c'est le défaut, et le niveau neutre du template du modèle. */
export const NIVEAUX_PENSEE = ["moyenne", "courte", "profonde", "non"] as const;
export type NiveauPensee = (typeof NIVEAUX_PENSEE)[number];

export function ChatPleinEcran({
  store, ouvert, onFermer,
}: {
  store: string;
  ouvert: boolean;
  onFermer: () => void;
}) {
  const [sujet] = useAtom(subjectAtom);
  /**
   * ── L'ÉTAT DEVIENT UN CAHIER DE CONVERSATIONS (`conversations.ts`) ─────────
   *
   * Avant : UNE liste de tours par store (`obrain.chat.v1.<store>`). Conséquence
   * mesurable : poser une question écrasait la conversation précédente, aucune
   * conversation n'avait de nom, et rien ne se rangeait — « structurer » était
   * impossible, il n'existait qu'un fil unique et un bouton pour tout oublier.
   * Ici : plusieurs conversations, chacune titrée par sa PREMIÈRE question,
   * rangeable dans un dossier, épinglable. L'ancienne clé est LUE et ADOPTÉE
   * (`migrer`) — jamais réécrite : on ne perd pas ses échanges en changeant de
   * format.
   *
   * ── LE CAHIER PORTE LA PORTÉE (`porteeDe`), IL NE PORTE PAS LA BASE SEULE ──
   * La clé du cahier n'est pas le store mais la PORTÉE (self → compte → base
   * provisoire), apport de la lignée canonique : plusieurs personnes peuvent
   * ouvrir le même navigateur, et chacune a SON self. Le cahier d'un compte
   * reste donc le sien — plusieurs conversations PAR self, et non un seul fil.
   */
  /**
   * Le tour du CHAT, vu par le cahier. `conversations.ts` exige une indexation
   * (`TourMin`) que l'interface `Tour` de cette vue n'a pas : on l'ajoute ICI,
   * au point de rencontre, plutôt que de desserrer le module pour tout le monde.
   */
  /**
   * LE SELF DE CETTE CONVERSATION, ANNONCÉ PAR LE SERVEUR.
   *
   * On ne le DEVINE pas : c'est le serveur qui le résout (`self_qn_pour`, par
   * l'identité de l'appelant). Il arrive dans l'événement `contexte` qui suit la
   * réflexion du modèle. Dès qu'on l'a, le cahier se range sous SON nom — et un
   * autre compte, sur le même navigateur, ne peut plus tomber sur ces échanges.
   */
  const [selfQn, setSelfQn] = useState<string | null>(null);
  /**
   * LE COMPTE, ANNONCÉ DÈS L'OUVERTURE DU FLUX (`event: identite`).
   *
   * Le `self_qn` n'arrive qu'avec `contexte`, donc après la réflexion du modèle :
   * plusieurs dizaines de secondes pendant lesquelles l'écran ne saurait pas à
   * qui appartient la conversation. Le COMPTE, lui, est connu du serveur avant
   * tout appel au modèle — et c'est lui qui distingue deux personnes sur un même
   * navigateur. On range donc sous le compte d'abord, sous le self ensuite.
   */
  const [compte, setCompte] = useState<string | null>(null);
  /**
   * ── LA PORTÉE DU CAHIER, TENUE EN ÉTAT (et non recalculée) ────────────────
   *
   * `portee = porteeDe(selfQn, compte, store)` serait recalculé à chaque rendu :
   * on ne pourrait alors plus distinguer « la portée a changé » de « le rendu a
   * eu lieu », et la relecture deviendrait un effet de bord permanent. On garde
   * donc la portée SOUS LAQUELLE LE CAHIER A ÉTÉ LU, et on ne relit que
   * lorsqu'elle change réellement.
   */
  const [portee, setPortee] = useState<string>(() => porteeDe(null, null, store));
  /**
   * ── CHANGER D'ESPACE — OU D'IDENTITÉ — CHANGE DE CAHIER (deux défauts MESURÉS) ─
   *
   * `useState(() => chargerEtat(portee))` ne lit le disque qu'UNE fois, au montage —
   * et au montage `store` vaut souvent "" (le sujet n'est pas encore résolu : c'est
   * `sujetChat?.store || active || ""`) : l'effet de sauvegarde ÉCRIVAIT alors dans
   * la clé du vrai espace un état chargé depuis la clé VIDE. Mesuré, clés
   * `localStorage` à l'appui : deux clés vivaient en parallèle, et une conversation
   * créée (271 o = 2 conversations) retombait à 155 o (1 conversation) après un
   * simple rechargement — la conversation était ÉCRASÉE par celle du store vide.
   * Un cahier appartient à un espace ET à un self : quand la portée change — le
   * self ou le compte vient d'être annoncé, ou l'espace a changé — on DÉMÉNAGE ce
   * qui a été écrit sous la portée provisoire, puis on RELIT.
   *
   * Sans cela, structurer ses échanges reviendrait à les perdre en changeant
   * d'espace ; sans la portée, à les rendre au compte suivant.
   */
  const [cahier, setCahier] = useState<Etat<TourC>>(() => chargerEtat(porteeDe(null, null, store)));
  useEffect(() => {
    const neuve = porteeDe(selfQn, compte, store);
    setPortee((ancienne) => {
      if (ancienne === neuve) return ancienne;
      migrerPortee(porteeDeCle(ancienne), porteeDeCle(neuve));
      return neuve;
    });
  }, [selfQn, compte, store]);
  useEffect(() => {
    // On ne lit JAMAIS la portée courante : l'effet ci-dessus vient de la poser,
    // et relire ici écraserait un cahier que l'utilisateur est en train d'écrire.
    if (porteeDeCle(portee) === porteeDe(selfQn, compte, store)) return;
    setCahier(chargerEtat(porteeDeCle(portee)));
  }, [portee, selfQn, compte, store]);
  const conv = courante(cahier);
  const tours = conv?.tours ?? [];
  const [listeOuverte, setListeOuverte] = useState(false);
  /** Le geste d'écriture de la vue : il n'écrit QUE dans la conversation ouverte. */
  const majTours = useCallback((f: (t: TourC[]) => TourC[]) => {
    setCahier((e) => majCourante(e, f(courante(e)?.tours ?? [])));
  }, []);
  // Persisté après chaque TOUR TERMINÉ — jamais pendant le flux : écrire à
  // chaque jeton ferait payer un `setItem` par delta. `aPersister` retire les
  // tours EN COURS (un tour « en cours » relu ferait tourner une attente
  // éternelle) et borne à 40 par conversation.
  useEffect(() => {
    // On n'écrit QUE sous la portée dont ce cahier vient : sinon un état chargé
    // d'ailleurs écraserait la clé de l'identité courante (défaut mesuré plus haut).
    if (porteeDeCle(portee) !== porteeDe(selfQn, compte, store)) return;
    if (courante(cahier)?.tours.some((t) => t.encours)) return;
    try {
      localStorage.setItem(cleEtat(portee), JSON.stringify(aPersister(cahier)));
    } catch { /* quota ou stockage refusé : la conversation ne doit pas échouer pour ça */ }
  }, [cahier, portee, selfQn, compte, store]);
  const [question, setQuestion] = useState("");
  const [enCours, setEnCours] = useState(false);
  const [voirThink, setVoirThink] = useState(true);
  /**
   * ── LES RÉGLAGES SE REPLIENT SUR PETIT ÉCRAN, ILS NE DISPARAISSENT PAS ─────
   *
   * Mesuré : à 320 px de large, l'en-tête ne peut pas porter cinq éléments en
   * restant lisible (11 px minimum) et tactile (44 px). Deux tentatives ont
   * échoué — cibles réduites à 33 px, puis texte à 10,5 px. La bonne réponse est
   * de RETIRER des éléments, mais un réglage retiré sans recours est un
   * cul-de-sac : le bouton ci-dessous les rappelle. Visible seulement sous
   * 400 px (en CSS), il ne prend aucune place sur un ordinateur.
   */
  const [reglagesOuverts, setReglagesOuverts] = useState(false);
  /**
   * LE NIVEAU DE RAISONNEMENT — CHOISI, ET CONSERVÉ.
   *
   * Défaut `moyenne` : c'est le niveau NEUTRE du template du modèle. Avant ce
   * réglage, le chat n'envoyait rien et le template appliquait `xhigh` — son
   * propre défaut, dont le texte ordonne « think carefully, validate
   * assumptions, consider plausible alternatives » : 38,6 s mesurés pour une
   * question de comptage. `moyenne` donne la même réponse juste en 25,2 s,
   * `courte` (low) en 27,0 s, `non` en 4,3 s — l'outil étant appelé dans les
   * quatre cas.
   *
   * Conservé par navigateur (comme l'historique) : un réglage qu'il faut
   * refaire à chaque ouverture ne sert à rien.
   */
  const [pensee, setPensee] = useState<NiveauPensee>(() => {
    try {
      const garde = localStorage.getItem("obrain.chat.pensee");
      // Ce qui n'est pas un niveau connu retombe sur `moyenne` : une valeur
      // héritée ou bricolée ne doit pas faire échouer la requête.
      return (NIVEAUX_PENSEE as readonly string[]).includes(garde || "")
        ? (garde as NiveauPensee) : "moyenne";
    } catch { return "moyenne"; }
  });
  useEffect(() => {
    try { localStorage.setItem("obrain.chat.pensee", pensee); } catch { /* stockage refusé */ }
  }, [pensee]);
  /**
   * LES RÉPARATIONS DUES, AFFICHÉES.
   *
   * Le protocole a trois temps : le producteur attache (pendant que la base
   * tourne et que la réparation est impossible), le consommateur applique et
   * accorde (à la fenêtre du redémarrage), et l'interface le MONTRE. Un protocole
   * qui n'est visible que dans un terminal n'en est pas un : une réparation due
   * depuis trois jours doit se voir, avec son âge. Sans cela elle attend en
   * silence — et une attente silencieuse ne se distingue pas d'un oubli.
   */
  const [entretien, setEntretien] = useState<Entretien | null>(null);
  useEffect(() => {
    if (!ouvert) return;
    const ctl = new AbortController();
    const lire = () =>
      fetchEntretien(ctl.signal)
        .then(setEntretien)
        .catch((e) => { if (!isAbort(e)) setEntretien(null); });
    lire();
    const t = window.setInterval(lire, 30_000);
    return () => { ctl.abort(); window.clearInterval(t); };
  }, [ouvert]);
  const sourceRef = useRef<EventSource | null>(null);
  const corpsRef = useRef<HTMLDivElement | null>(null);
  /** Le chat est-il encore ouvert ? Une reprise automatique ne doit pas
   *  relancer une question dans une conversation qu'on vient de fermer. */
  const ouvertRef = useRef(ouvert);
  useEffect(() => { ouvertRef.current = ouvert; }, [ouvert]);
  /**
   * LES TOURS, EN DIRECT.
   *
   * Le gestionnaire d'erreur d'un `EventSource` est posé à l'OUVERTURE du flux :
   * il ne voit donc que le `tours` du rendu où la question a été posée. Une
   * reprise a besoin de l'histoire DÉJÀ ÉCRITE (elle est renvoyée au serveur pour
   * qu'il continue, et pas qu'il recommence) — d'où cette référence.
   */
  const toursRef = useRef<Tour[]>(tours);
  useEffect(() => { toursRef.current = tours; }, [tours]);
  /** L'utilisateur suit-il le bas ? S'il remonte pour relire, on ne le ramène
   *  pas de force : pendant un flux, la vue sautait en bas à chaque morceau. */
  const collant = useRef(true);

  // ── UN RENDU PAR IMAGE, PAS UN PAR MORCEAU ────────────────────────────────
  // Le serveur pousse un événement SSE par morceau de modèle (~30 ms). Chaque
  // morceau déclenchait un `setState`, donc un rendu React complet — pour trois
  // caractères. On tamponne et on vide le tampon une fois par image : le texte
  // affiché est identique, le travail est divisé par le nombre de morceaux
  // arrivés entre deux images. Le dernier morceau n'est jamais perdu : `fin` et
  // `erreur` vident le tampon AVANT de conclure.
  const tampon = useRef({ texte: "", think: "" });
  const raf = useRef<number | null>(null);
  /**
   * LE TEXTE DÉJÀ APPLIQUÉ À L'ÉCRAN, tenu ICI et non lu dans `tours`.
   *
   * Une reprise renvoie au serveur ce qui a été affiché pour qu'il CONTINUE la
   * réponse. Lire `tours` serait lire l'état du dernier rendu (les effets React
   * passent après le commit) : les mots arrivés entre-temps seraient perdus, et le
   * modèle les rejouerait — une duplication au raccord, exactement ce qu'on veut
   * éviter. Cette référence est mise à jour à l'instant où un morceau est
   * appliqué, donc elle ne peut pas retarder.
   */
  const texteAffiche = useRef("");

  const majAssistant = useCallback((f: (t: TourC) => TourC) => {
    majTours((t) => {
      const copie = [...t];
      for (let i = copie.length - 1; i >= 0; i--) {
        if (copie[i].role === "assistant") { copie[i] = f(copie[i]); break; }
      }
      return copie;
    });
  }, []);

  const appliquerTampon = useCallback(() => {
    const { texte, think } = tampon.current;
    if (!texte && !think) return;
    tampon.current = { texte: "", think: "" };
    if (texte) texteAffiche.current += texte;
    majAssistant((t) => ({
      ...t,
      texte: t.texte + texte,
      think: think ? (t.think || "") + think : t.think,
    }));
  }, [majAssistant]);

  /** Applique tout de suite ce qui est en tampon (fin de flux, erreur, envoi). */
  const viderTampon = useCallback(() => {
    if (raf.current !== null) { cancelAnimationFrame(raf.current); raf.current = null; }
    appliquerTampon();
  }, [appliquerTampon]);

  const pousser = useCallback((quoi: "texte" | "think", morceau: string) => {
    if (!morceau) return;
    tampon.current[quoi] += morceau;
    if (raf.current !== null) return;
    raf.current = requestAnimationFrame(() => { raf.current = null; appliquerTampon(); });
  }, [appliquerTampon]);

  // Le bas de la conversation suit le flux — mais seulement si on y est déjà.
  useEffect(() => {
    const el = corpsRef.current;
    if (el && collant.current) el.scrollTop = el.scrollHeight;
  }, [tours]);

  const surDefilement = () => {
    const el = corpsRef.current;
    if (el) collant.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };

  // La source est fermée dès qu'on quitte : une conversation laissée ouverte
  // continuerait de pousser dans le vide.
  useEffect(() => {
    if (!ouvert) {
      sourceRef.current?.close();
      sourceRef.current = null;
      if (raf.current !== null) { cancelAnimationFrame(raf.current); raf.current = null; }
      tampon.current = { texte: "", think: "" };
      setEnCours(false);
    }
  }, [ouvert]);

  const historique = useMemo(
    () => tours.filter((t) => !t.erreur).map((t) => [t.role, t.texte] as [string, string]),
    [tours],
  );

  // L'ANCRAGE RÉSOLU, tel que le serveur l'a RAPPORTÉ — jamais tel que la vue
  // le suppose. C'est la correction du défaut mesuré : l'en-tête affichait un
  // monde et l'engine en lisait un autre. Désormais l'en-tête montre ce qui a
  // été ouvert, et il ne peut plus en diverger.
  const resolu = useMemo(() => {
    for (let i = tours.length - 1; i >= 0; i--) {
      if (tours[i].contexte?.store) return tours[i].contexte!;
    }
    return null;
  }, [tours]);

  /**
   * L'HISTORIQUE D'UNE REPRISE — SANS LE TOUR EN COURS.
   *
   * Le tour en cours est transmis À PART (paramètre `deja`) : le laisser aussi
   * dans l'historique ferait lire au modèle deux fois la même réponse partielle,
   * et il recommencerait au lieu de continuer. On retire donc le tour assistant
   * qu'on reprend et la question qui l'a produit — la question repart séparément.
   */
  const historiqueDeReprise = (): [string, string][] => {
    const courant = toursRef.current;
    const paires: [string, string][] = [];
    for (const t of courant) {
      if (t.encours) break; // le tour en cours, et ce qui suit, n'est pas de l'histoire
      if (t.erreur) continue;
      paires.push([t.role, t.texte]);
    }
    if (paires.length && paires[paires.length - 1][0] === "user") paires.pop();
    return paires.slice(-8);
  };

  /**
   * LE TEXTE DÉJÀ AFFICHÉ DU TOUR EN COURS — ce que le serveur doit CONTINUER.
   * Les morceaux appliqués (`texteAffiche`) PLUS ceux encore en vol (le tampon
   * d'image) : sinon le raccord laisserait tomber les derniers mots reçus.
   */
  const texteEnCours = (): string => texteAffiche.current + tampon.current.texte;

  /**
   * OUVRE LE FLUX D'UNE QUESTION DÉJÀ POSÉE.
   *
   * `opts.deja` = le texte DÉJÀ produit : le serveur le rejoue au modèle et lui
   * demande de CONTINUER. C'est ce qui fait d'une coupure de transport une
   * interruption au lieu d'une réponse perdue.
   */
  const lancerFlux = (
    q: string,
    opts: { deja?: string; tentative?: number; historique?: [string, string][] } = {},
  ) => {
    /**
     * ── UN CHAT N'EXIGE PAS UNE BASE POUR PARLER ──────────────────────────
     *
     * Ici commençait un `if (!store) return;` : sans espace visé, la question
     * n'était même pas envoyée. C'est ce qui faisait de ce panneau une console
     * SUR le graphe plutôt qu'un chat — et c'est exactement le cas de la PREMIÈRE
     * CONNEXION (compte sans espace accordé), où l'on a le plus besoin de parler.
     * Or le SERVEUR sait répondre sans qu'on lui nomme une base : sans `store`,
     * `ancrer_sujet` retombe sur l'espace par défaut de l'instance, et le Self
     * vient du COMPTE connecté (`compte_connecte` → `self_qn_pour`). Le refus était
     * donc plus strict que le serveur, et il fermait la porte au seul endroit où
     * elle sert. On n'envoie simplement PAS le paramètre quand il n'y a rien à
     * nommer — `store=` vide serait pire : le serveur le lirait comme un chemin vide.
     */
    const params = new URLSearchParams({ q });
    if (store) params.set("store", store);
    if (sujet?.qn) params.set("qn", sujet.qn);
    // ── LE NIVEAU DE RAISONNEMENT PART AVEC LA QUESTION ─────────────────────
    // Défaut mesuré (2026-09-15) AVANT ce réglage : le chat n'envoyait rien au
    // template, qui appliquait donc son propre défaut `xhigh` — 38,6 s de
    // raisonnement pour une question de comptage. Le même choix en `moyenne`
    // rend la MÊME réponse juste en 25,2 s ; `non` en 4,3 s, l'outil étant
    // appelé quand même.
    params.set("pensee", pensee);
    const h = (opts.historique ?? historique).slice(-8);
    if (h.length) params.set("h", JSON.stringify(h));
    if (opts.deja) params.set("deja", opts.deja);
    const tentative = opts.tentative ?? 0;
    const src = new EventSource(`/api/engine/chat?${params.toString()}`);
    sourceRef.current = src;
    /** Le serveur a-t-il dit quelque chose de la RÉPONSE (think ou texte) ? */
    let aRepondu = false;
    // ── LE RACCORD D'UNE REPRISE ──────────────────────────────────────────
    // On retient le DÉBUT de la suite jusqu'à `ATTENTE_RACCORD` caractères : c'est
    // ce qui permet de savoir si le modèle CONTINUE la réponse ou la RECOMMENCE.
    // Les deux cas ont été mesurés le même jour, sur la même question (« 8, 19, 20 »
    // d'un côté, le compte repris de « 1, 2, 3 » de l'autre) — voir `raccord.ts`.
    let raccord: string | null = opts.deja ? "" : null;

    /** Applique ce qui est retenu pour le raccord (fin de flux, ou reprise). */
    const viderRaccord = () => {
      if (raccord === null) return;
      const r = raccorder(opts.deja ?? "", raccord);
      raccord = null;
      if (r.remplace) {
        // La suite est une réponse ENTIÈRE : elle REMPLACE le texte affiché. Le
        // montrer deux fois serait un doublon, garder l'ancien tronqué serait
        // mentir sur ce que le modèle vient de produire.
        texteAffiche.current = r.ajout;
        majAssistant((c) => ({ ...c, texte: r.ajout, repriseComplete: true, reprises: c.reprises }));
      } else if (r.ajout) {
        pousser("texte", r.ajout);
      }
    };

    /**
     * ── UNE COUPURE N'EST PAS UNE FIN : ON REPREND ────────────────────────
     *
     * Défaut mesuré le 2026-09-13 : le service `viz serve` est tué en pleine
     * réponse par un déploiement (journal systemd : `status=9/KILL`, 1 248
     * redémarrages comptés), la connexion SSE meurt, et la vue n'avait plus qu'à
     * écrire « flux interrompu avant la fin — ce qui précède est incomplet ». Une
     * panne de TRANSPORT devenait une réponse inutilisable, alors que le texte
     * déjà affiché est connu ici et qu'un modèle sait continuer une réponse qu'on
     * lui remontre. On rouvre donc le flux avec ce texte : la réponse CONTINUE.
     * Bornée — au-delà, on le DIT et on laisse le bouton « reprendre », parce
     * qu'insister sans fin contre un service absent est une boucle.
     */
    const reprendre = (cause: string) => {
      // Le raccord et le tampon sont soldés AVANT de lire le texte : ce qui n'est
      // pas encore appliqué ne doit pas être perdu par la reprise (il serait
      // redemandé au modèle, donc écrit deux fois).
      viderRaccord();
      viderTampon();
      const deja = texteEnCours();
      if (sourceRef.current === src) sourceRef.current = null;
      src.close();
      if (tentative >= MAX_REPRISES) {
        const base =
          `${cause}. ${tentative} reprise(s) automatique(s) n'ont pas abouti — la réponse ` +
          `s'arrête ici : « reprendre » la relance quand le service est revenu.`;
        majAssistant((t) => ({
          ...t,
          encours: false,
          reprise: false,
          erreurReprenable: true,
          erreur: base,
        }));
        setEnCours(false);
        // LA CAUSE EST VÉRIFIÉE, PAS DEVINÉE (voir `diagnostiquerCoupure`) : la
        // reprise a échoué plusieurs fois — autant dire laquelle des causes
        // réelles c'est, au lieu de laisser l'utilisateur chercher.
        void diagnostiquerCoupure().then((verite) =>
          majAssistant((t) => (t.erreur === base ? { ...t, erreur: `${base} ${verite}` } : t)),
        );
        return;
      }
      majAssistant((t) => ({
        ...t,
        reprise: true,
        encours: true,
        reprises: (t.reprises || 0) + 1,
      }));
      setEnCours(true);
      window.setTimeout(() => {
        if (sourceRef.current === null && ouvertRef.current) {
          lancerFlux(q, { deja, tentative: tentative + 1, historique: historiqueDeReprise() });
        }
      }, DELAIS_REPRISE_MS[Math.min(tentative, DELAIS_REPRISE_MS.length - 1)]);
    };

    // LE COMPTE ARRIVE EN PREMIER — c'est la première trame du flux.
    src.addEventListener("identite", (ev) => {
      try {
        const d = JSON.parse((ev as MessageEvent).data);
        if (typeof d?.compte === "string" && d.compte) setCompte(d.compte);
      } catch { /* trame illisible : on garde le repli */ }
    });
    src.addEventListener("contexte", (ev) => {
      const d = JSON.parse((ev as MessageEvent).data);
      majAssistant((t) => ({ ...t, contexte: d }));
      // ── LE SELF ARRIVE ICI, ET IL DÉCIDE DE LA MÉMOIRE DE L'ÉCRAN ────────
      // On l'adopte pour ranger la conversation sous SON nom. On ne recharge
      // PAS l'historique de l'autre self : les tours affichés sont ceux de cette
      // session, et les remplacer sous les yeux de l'utilisateur serait un
      // changement d'écran qu'il n'a pas demandé.
      if (typeof d?.self_qn === "string" && d.self_qn) setSelfQn(d.self_qn);
    });
    // LES APPELS D'OUTILS ARRIVENT EN DIRECT. Les taire reviendrait à cacher le
    // travail : on ne saurait pas si la densification a eu lieu, ni sur quoi.
    src.addEventListener("outil", (ev) => {
      const d = JSON.parse((ev as MessageEvent).data) as OutilAppel;
      majAssistant((t) => ({ ...t, outils: [...(t.outils || []), d] }));
    });
    // LA MÉMOIRE ARRIVE EN DIRECT — et séparément des lectures : c'est une
    // écriture journalisée, pas une consultation.
    src.addEventListener("memoire", (ev) => {
      const d = JSON.parse((ev as MessageEvent).data) as Memoire;
      majAssistant((t) => ({ ...t, memoires: [...(t.memoires || []), d] }));
    });
    src.addEventListener("think", (ev) => {
      const d = JSON.parse((ev as MessageEvent).data);
      aRepondu = true;
      pousser("think", d.t);
    });
    src.addEventListener("texte", (ev) => {
      const d = JSON.parse((ev as MessageEvent).data);
      aRepondu = true;
      if (raccord === null) {
        pousser("texte", d.t);
        return;
      }
      raccord += d.t;
      if (raccord.length < ATTENTE_RACCORD) return; // on patiente pour décider
      viderRaccord();
    });
    src.addEventListener("fin", (ev) => {
      const d = JSON.parse((ev as MessageEvent).data);
      aRepondu = true;
      viderTampon(); // sans ça, les derniers morceaux resteraient en tampon
      viderRaccord(); // un flux court peut finir avant la décision du raccord
      // ── LE MODÈLE S'EST ARRÊTÉ SANS DIRE QU'IL AVAIT FINI ────────────────
      // Le service de modèle a coupé son flux sans marqueur de fin : `post_sse`
      // rendait `Ok` sur une fermeture, donc la coupure passait pour une lecture
      // réussie et le texte partiel se lisait comme une réponse COMPLÈTE. Le
      // serveur NOMME désormais cette fin (`raison: "flux_coupe"`) : on continue
      // la réponse au lieu de la croire terminée.
      if (d.raison === "flux_coupe") {
        reprendre("la génération s'est interrompue avant la fin");
        return;
      }
      majAssistant((t) => ({
        ...t,
        ms_llm: d.ms_llm,
        raison: d.raison ?? null,
        encours: false,
        reprise: false, // la reprise a abouti : la mention n'a plus lieu d'être
        // Un flux qui se termine « proprement » SANS un seul caractère n'est pas
        // une réponse : c'est une panne déguisée (requête refusée ou coupée
        // avant le premier mot). Mesuré le 2026-09-11 sur la base « Runtime
        // dev » : contexte annoncé, zéro texte, zéro événement d'erreur — le
        // vide se lisait comme un modèle sans avis. On le dit, et on le rejoue.
        erreur: !t.texte && !t.think && !t.erreur
          ? "le modèle a terminé sans produire aucun texte : la requête a été refusée ou coupée avant le premier mot (journal du serveur pour la cause)"
          : t.erreur,
        erreurReprenable: !t.texte && !t.think ? true : t.erreurReprenable,
      }));
      setEnCours(false);
      src.close();
    });
    src.addEventListener("erreur", (ev) => {
      let msg = "le flux a été interrompu par le serveur";
      try { msg = JSON.parse((ev as MessageEvent).data).error || msg; } catch { /* brut */ }
      aRepondu = true; // le serveur a parlé : ce n'est pas une coupure muette
      viderTampon();
      // ── L'ERREUR DU SERVEUR SE DIT, ET SE REJOUE ─────────────────────────
      // Un refus du service de modèle (fenêtre dépassée, requête invalide) n'est
      // PAS une coupure de transport : le masquer par une reprise silencieuse
      // serait le mensonge que tout ce fichier corrige. On l'affiche — mais avec
      // de quoi la rejouer, parce qu'une réponse qui s'arrête sans recours est
      // précisément ce qu'on corrige.
      majAssistant((t) => ({
        ...t,
        erreur: msg,
        erreurReprenable: true,
        encours: false,
        reprise: false,
      }));
      setEnCours(false);
      src.close();
    });
    // `onerror` se déclenche AUSSI quand la source se ferme, et le chemin normal
    // passe par `fin`, qui la ferme lui-même : cet `onerror` ne voit donc que les
    // VRAIES coupures. Elles se reprennent TOUTES — avant comme après le premier
    // mot. C'est la différence avec la version précédente : une coupure APRÈS du
    // texte était marquée « interrompu » et l'utilisateur lisait « ce qui précède
    // est incomplet ». On ne s'arrête qu'après `MAX_REPRISES` tentatives — et on
    // le dit alors, avec le bouton pour rejouer.
    src.onerror = () => {
      reprendre(
        aRepondu
          ? "le flux a été coupé en pleine réponse (service redémarré, ou réseau)"
          : "le flux s'est fermé avant la première réponse",
      );
    };
  };

  /**
   * REPRENDRE À LA MAIN — parce qu'un échec doit toujours pouvoir être rejoué.
   *
   * La reprise automatique est bornée (contre un service absent, insister
   * bouclerait) ; au-delà, c'est l'utilisateur qui décide — et le bouton dit
   * exactement ce qu'il fait : CONTINUER la réponse, pas la recommencer.
   */
  const reprendreLaMain = (q: string) => {
    if (enCours) return;
    setEnCours(true);
    collant.current = true;
    majAssistant((t) => ({
      ...t,
      erreur: undefined,
      erreurReprenable: false,
      reprise: true,
      encours: true,
      reprises: (t.reprises || 0) + 1,
    }));
    lancerFlux(q, { deja: texteEnCours(), tentative: 0, historique: historiqueDeReprise() });
  };

  const envoyer = () => {
    const q = question.trim();
    // `!store` n'est plus une raison de ne pas envoyer (voir `lancerFlux`) : sans
    // espace, le serveur ancre sur le sien et le Self du compte.
    if (!q || enCours) return;
    setQuestion("");
    setEnCours(true);
    collant.current = true; // une question qu'on vient de poser : on suit
    texteAffiche.current = ""; // un tour NEUF : le texte de reprise repart de zéro
    // Le tour de l'assistant est créé VIDE : il se remplit au fil du flux — et il
    // PORTE sa question, parce que c'est elle qu'une reprise repose à l'identique.
    majTours((t) => [
      ...t,
      { role: "user", texte: q },
      { role: "assistant", texte: "", encours: true, question: q },
    ]);
    lancerFlux(q);
  };

  if (!ouvert) return null;

  return (
    <div className="chat-plein">
      <div className={"chat-tete" + (reglagesOuverts ? " reglages-ouverts" : "")}>
        {/* ── LE TITRE DIT CE QUE LE CHAT EST RÉELLEMENT ────────────────────
            « l'engine sur le graphe » était vrai tant qu'un espace était visé.
            Sans espace (première connexion), il n'y a pas de graphe global à
            interroger : l'annoncer serait l'écart entre l'intention et ce qui est
            lu — précisément ce que ce panneau corrige ailleurs. */}
        <b>{store ? "Chat — l'engine sur le graphe" : "Chat — l'engine (sans graphe visé)"}</b>
        <span className="chat-sujet">
          {!store
            ? <>— aucun espace accordé : la réponse vient de l'espace par défaut du serveur et de votre <b>Self</b></>
            : sujet?.qn ? <>ancré sur <b>{sujet.label || sujet.qn}</b></> : <>ancré sur le <b>graphe global</b></>}
          {/* CE QUI A ÉTÉ OUVERT, rapporté par le serveur. Sans cette mention,
              l'en-tête n'est qu'une intention : c'est précisément l'écart entre
              les deux qui produisait une réponse confiante sur la mauvaise base. */}
          {resolu?.store ? (
            <> · lu dans <b title={resolu.store}>{nomStore(resolu.store)}</b></>
          ) : null}
        </span>
        {/* CE QUI EST DÛ — avec l'âge de la plus ancienne. */}
        {entretien && entretien.en_attente > 0 ? (
          <span
            className="chip on"
            title={
              "Réparations attachées en vol, en attente de la fenêtre d'exclusivité " +
              "(elles s'appliquent au prochain redémarrage) :\n" +
              entretien.demandes
                .map((d) => `• ${d.store_court} / ${d.genre} — ${d.raison.slice(0, 90)} (il y a ${Math.round(d.age_s / 60)} min)`)
                .join("\n")
            }
          >
            ⚠ {entretien.en_attente} réparation{entretien.en_attente > 1 ? "s" : ""} due
            {entretien.en_attente > 1 ? "s" : ""}
            {entretien.age_max_s ? ` · ${Math.round(entretien.age_max_s / 60)} min` : ""}
          </span>
        ) : null}
        {/* LE RAPPEL DES RÉGLAGES — n'apparaît que là où ils sont repliés. */}
        <button className={"chip chat-reglages-bouton" + (reglagesOuverts ? " on" : "")}
          onClick={() => setReglagesOuverts((v) => !v)}
          aria-expanded={reglagesOuverts}
          title="Montrer ou cacher les réglages de la conversation">
          ⚙
        </button>
        <button className="chip chat-reglage" onClick={() => setVoirThink((v) => !v)}
          title="Afficher ou masquer le raisonnement du modèle">
          {voirThink ? "think visible" : "think masqué"}
        </button>
        {/* LE NIVEAU DE RAISONNEMENT SE CHOISIT ICI. Il ne se subit plus : le
            défaut du template (`xhigh`) coûtait 38,6 s mesurés pour une question
            de comptage, contre 25,2 s en `moyenne` et 4,3 s sans pensée — la
            réponse restant juste, outil compris. */}
        <button className="chip"
          onClick={() => setPensee(NIVEAUX_PENSEE[(NIVEAUX_PENSEE.indexOf(pensee) + 1) % NIVEAUX_PENSEE.length])}
          title="Niveau de raisonnement : moyenne (défaut, niveau neutre du modèle) · courte · profonde (le plus lent) · aucune (le plus rapide)">
          pensée : {pensee}
        </button>
        {/* L'HISTORIQUE CONSERVÉ SE VOIT, ET SE VIDE. Un historique qu'on ne peut
            pas vider est une gêne ; un historique qu'on ne sait pas conservé est
            une perte qu'on découvre au rechargement. */}
        {tours.length > 0 && (
          <button className="chip" title={`${tours.length} tour(s) conservé(s) par ce navigateur pour ce store — cliquer pour les oublier`}
            onClick={() => {
              // ── LA CLÉ EST CELLE DU PROPRIÉTAIRE DU CAHIER (lignée canonique) ──
              // Effacer `store` nu ne touchait pas l'emplacement réellement écrit
              // (la portée porte le self, puis le compte) : l'historique
              // disparaissait de l'écran et revenait au rechargement, ce qui est
              // pire qu'un bouton absent.
              // ── CE GESTE VIDE L'ÉCRAN, PAS LES AUTRES CONVERSATIONS ─────────
              // `conversations.ts` a établi que les conversations d'un même
              // propriétaire sont SÉPARÉES : vider le stockage les effacerait
              // toutes sous couvert d'un bouton qui n'en montre qu'une. On vide
              // donc la conversation ouverte ; les autres se suppriment une à une
              // depuis la liste (🗂), où chacune est nommée.
              oublierHistorique(portee); majTours(() => []);
            }}>
            historique ({tours.length}) · vider
          </button>
        )}
        <button className={"chip" + (listeOuverte ? " on" : "")}
          onClick={() => setListeOuverte((v) => !v)}
          aria-expanded={listeOuverte}
          title="Vos conversations pour cet espace : en ouvrir une, en commencer une, les nommer, les ranger"
        >🗂 {cahier.liste.length}</button>
        <button className="chip" onClick={onFermer} title="Fermer la conversation">✕</button>
      </div>

      {/* ── STRUCTURER SES CONVERSATIONS ─────────────────────────────────────
          Avant ce bloc, il n'existait qu'UNE conversation par espace : la
          nouvelle question écrasait la précédente, rien n'avait de nom, et il
          n'y avait AUCUN geste pour ranger. Ici : la liste (épinglées d'abord,
          puis la plus récente), ce que chacune contient, son dossier, et les
          gestes — ouvrir, commencer, nommer, ranger, épingler, supprimer. Le tri
          et les invariants vivent dans `conversations.ts`, testé sans navigateur. */}
      {listeOuverte && (
        <div className="chat-convs" role="list">
          <button className="chip on" onClick={() => { setCahier((e) => ajouter(e)); setQuestion(""); }}
            title="Commencer une conversation — la précédente est CONSERVÉE">＋ nouvelle conversation</button>
          {dossiers(cahier).filter((d) => d.nom).length > 0 && (
            <span className="q-intro" style={{ opacity: 0.8, marginLeft: 6 }}>
              dossiers : {dossiers(cahier).filter((d) => d.nom).map((d) => `${d.nom} (${d.n})`).join(" · ")}
            </span>
          )}
          {trier(cahier.liste).map((c) => {
            const nom = c.titre || titreAuto(c.tours) || "sans titre";
            const ouverte = c.id === cahier.courante;
            return (
              <div role="listitem" key={c.id} className={"chat-conv" + (ouverte ? " on" : "")}>
                <button className="chat-conv-ouvrir"
                  onClick={() => { setCahier((e) => choisir(e, c.id)); setListeOuverte(false); }}
                  title={`Ouvrir « ${nom} » — ${c.tours.length} tour(s)${c.dossier ? `, dossier ${c.dossier}` : ""}`}>
                  <b>{nom}</b>
                  <span className="chat-conv-meta">
                    {c.epingle ? "📌 " : ""}{c.tours.length} tour{c.tours.length > 1 ? "s" : ""}
                    {c.dossier ? ` · 🗂 ${c.dossier}` : ""}
                    {" · "}{Math.max(1, Math.round((Date.now() - c.maj) / 60000))} min
                  </span>
                </button>
                <button className="chip" title="Renommer cette conversation"
                  onClick={() => {
                    const t = window.prompt("Nom de la conversation :", c.titre || titreAuto(c.tours) || "");
                    if (t !== null) setCahier((e) => renommer(e, c.id, t));
                  }}>✎</button>
                <button className="chip" title="Ranger dans un dossier — créer un nom le crée"
                  onClick={() => {
                    const existants = dossiers(cahier).filter((d) => d.nom).map((d) => d.nom);
                    const d = window.prompt(
                      "Dossier" + (existants.length ? ` (existants : ${existants.join(", ")})` : " (libre : « noyau », « clients »…)") + " — vide pour ne plus ranger :",
                      c.dossier);
                    if (d !== null) setCahier((e) => ranger(e, c.id, d));
                  }}>🗂</button>
                <button className={"chip" + (c.epingle ? " on" : "")}
                  title={c.epingle ? "Ne plus épingler" : "Épingler en tête"}
                  onClick={() => setCahier((e) => epingler(e, c.id))}>📌</button>
                <button className="chip"
                  title="Supprimer cette conversation (supprimer la dernière laisse une conversation neuve)"
                  onClick={() => {
                    if (window.confirm(`Supprimer « ${nom} » (${c.tours.length} tour(s)) ?`)) {
                      setCahier((e) => supprimer(e, c.id));
                    }
                  }}>🗑</button>
              </div>
            );
          })}
          <span className="q-intro" style={{ opacity: 0.7, display: "block", marginTop: 4 }}>
            rangées par épinglage puis par date · <b>{cahier.liste.length}</b> conversation(s)
          </span>
        </div>
      )}

      <div className="chat-corps" ref={corpsRef} onScroll={surDefilement}>
        {tours.length === 0 && (
          <div className="q-intro" style={{ opacity: 0.8 }}>
            Posez une question : le contexte obrain réellement lu s'affichera d'abord,
            puis le raisonnement, puis la réponse — au fil de l'eau.
          </div>
        )}
        {tours.map((t, i) => t.role === "user" ? (
          <div key={i} className="chat-moi">{t.texte}</div>
        ) : (
          <div key={i} className="chat-lui">
            {t.contexte && (
              <div className="chat-contexte">
                <div>
                  <b>obrain</b> · {partLue(t.contexte) || `${t.contexte.noeuds} nœuds`} ·{" "}
                  {t.contexte.relations} relations · lu en {t.contexte.ms} ms
                </div>
                {/* L'ANCRAGE RÉEL : le store ouvert, le monde, le self. */}
                <div className="chat-ancrage">
                  store : <code title={t.contexte.store}>{nomStore(t.contexte.store) || "—"}</code>
                  {t.contexte.monde ? <> · monde : <b>{t.contexte.monde}</b></> : null}
                  {t.contexte.self_qn ? <> · self : <code>{t.contexte.self_qn}</code></> : null}
                  {/* LA FENÊTRE EST DITE, avec sa provenance : elle commande la
                      taille du contexte, des résultats d'outils et le nombre de
                      tours. Un plafond invisible se lit comme une limite du
                      logiciel alors qu'il vient du service. */}
                  {t.contexte.fenetre ? (
                    <> · fenêtre : <b title={t.contexte.fenetre_source}>
                      {t.contexte.fenetre.toLocaleString("fr-FR")} jetons
                    </b></>
                  ) : null}
                </div>
                {/* LE CŒUR DU GRAPHE. Il était IMPOSSIBLE à montrer : le classement
                    par degré coûtait 14,1 s de parcours, et l'outil refusait la
                    requête. Les registres de degrés du substrat y répondent en
                    O(1) par nœud — l'entretien calcule le classement (6 ms sur
                    401 k nœuds) et on le relit ici. */}
                {t.contexte.hubs?.length ? (
                  <details className="chat-hubs">
                    <summary>
                      nœuds les plus connectés · {t.contexte.hubs.length} affichés
                      {t.contexte.hubs_ms ? <> · classement calculé en {t.contexte.hubs_ms} ms</> : null}
                    </summary>
                    <div>
                      {t.contexte.hubs.map((h, k) => (
                        <div key={k}><code>{h}</code></div>
                      ))}
                    </div>
                  </details>
                ) : null}
                {/* CE QUI RESTE À DENSIFIER : les types réellement présents, avec
                    leur volume et ce qui en a été lu. Le modèle peut demander la
                    suite (boucle d'outils) — l'utilisateur voit donc ce qu'il
                    reste sur la table au lieu de croire que tout est là. */}
                {t.contexte.par_label?.length ? (
                  // OUVERT PAR DÉFAUT : la répartition par type EST ce que veut
                  // dire « densification ». Repliée, elle serait un ornement de
                  // plus ; ouverte, elle dit exactement ce qui n'a pas été lu.
                  <details className="chat-densite" open>
                    <summary>
                      densification disponible · {t.contexte.par_label.length} types ·{" "}
                      {t.contexte.total_noeuds} nœuds au total
                      {t.contexte.total_aretes ? <> · {t.contexte.total_aretes} arêtes</> : null}
                    </summary>
                    <div>
                      {t.contexte.par_label.map((p) => (
                        <div key={p.label}>
                          <code>{p.label}</code> — {p.total} dans la base · {p.lus} lus
                        </div>
                      ))}
                    </div>
                  </details>
                ) : null}
                <i>{t.contexte.apercu}</i>
              </div>
            )}
            {/* LE RAISONNEMENT AUSSI : un modèle de raisonnement y écrit du
                markdown et du LaTeX comme dans sa réponse — le laisser en texte
                brut, c'était garder les antislashs d'un côté et pas de l'autre.
                Même moteur, mêmes blocs mémorisés.
                ⚠ LES APPELS D'OUTILS N'ONT PLUS LEUR BLOC ICI : ils sont DANS la
                réponse, à l'endroit où ils ont servi (voir `ReponseAvecOutils`). */}
            {t.memoires?.length ? (
              <div className="chat-memoire">
                {t.memoires.map((m, k) => (
                  <div key={k}>
                    <b>mémorisé</b> ({m.genre}) : {m.reference}
                    <span className="chat-memoire-attente"> · journalisé, en attente
                      d'application dans {m.self_qn}</span>
                  </div>
                ))}
              </div>
            ) : null}
            {voirThink && t.think ? (
              <details className="chat-think" open={!!t.encours}>
                <summary>raisonnement ({(t.think || "").length} caractères)</summary>
                <div className="chat-think-corps">
                  {/* `actif` ICI AUSSI : un raisonnement long est UN seul bloc
                      (il ne contient pas de ligne vide), donc le seul cas que le
                      repérage par blocs ne borne pas. Le gel retombe à la fin du
                      tour, où le texte final est rendu tel quel. */}
                  <MarkdownRiche texte={t.think} actif={t.encours} />
                </div>
              </details>
            ) : null}
            {/* LA RÉPONSE, EN MARKDOWN + LATEX : rendue par blocs mémorisés —
                un bloc déjà écrit n'est ni reparsé ni rendu à nouveau, donc une
                formule déjà affichée ne repasse jamais dans KaTeX. */}
            {/* LA RÉPONSE, EN MARKDOWN + LATEX, AVEC LES APPELS D'OUTILS DEDANS.
                Ils ne sont plus rangés dans un bloc à part (voir
                `ReponseAvecOutils`) : chacun s'affiche là où il a servi. */}
            {(t.texte || t.outils?.length) ? <ReponseAvecOutils t={t} /> : null}
            {t.encours && !t.texte && <div className="chat-attente">…</div>}
            {t.raison === "length" && (
              <div className="chat-tronque">
                réponse TRONQUÉE — le modèle a atteint sa limite de jetons, la fin
                manque. Reposez la question de façon plus précise, ou demandez une
                réponse plus courte.
              </div>
            )}
            {/* LA REPRISE SE DIT — ET ELLE A REMPLACÉ L'AVEU D'INACHEVÉ.
                La version précédente écrivait ici « flux interrompu avant la fin —
                ce qui précède est incomplet » : une panne de transport y devenait
                une réponse perdue. Une coupure est désormais REPRISE (le flux est
                rouvert avec le texte déjà affiché, et le serveur le fait
                continuer) ; on le dit, parce qu'une attente qui se prolonge sans
                explication passerait pour de la lenteur. */}
            {t.reprise && !t.erreur && (
              <div className="chat-meta">
                coupure du flux — reprise automatique
                {t.reprises ? ` (${t.reprises}/${MAX_REPRISES})` : ""} : la réponse CONTINUE
                au lieu d'être annoncée incomplète…
              </div>
            )}
            {/* ── LE MODÈLE A RÉGÉNÉRÉ LA RÉPONSE : ON LE DIT ───────────────
                Sans cette mention, le texte affiché changerait sous les yeux de
                l'utilisateur sans explication — et une réponse qui se remplace
                toute seule se lit comme un défaut. */}
            {t.repriseComplete && !t.erreur && (
              <div className="chat-meta">
                reprise : le modèle a RÉGÉNÉRÉ la réponse au lieu de la continuer — la
                version complète remplace le début affiché.
              </div>
            )}
            {t.ms_llm != null && (
              <div className="chat-meta">
                modèle : {(t.ms_llm / 1000).toFixed(1)} s
                {t.raison ? ` · fin : ${t.raison === "length" ? "plafond atteint" : t.raison}` : ""}
                {/* Ce qui a été repris se dit AUSSI APRÈS : sans cette mention, une
                    réponse recomposée en trois morceaux se lirait comme une seule
                    génération — et la prochaine coupure surprendrait encore. */}
                {t.reprises ? ` · réponse reprise ${t.reprises} fois après coupure` : ""}
              </div>
            )}
            {t.erreur && (
              <div className="chat-err">
                {t.erreur}
                {/* ── UN ÉCHEC DOIT POUVOIR ÊTRE REJOUÉ ────────────────────────
                    La reprise automatique est bornée ; le bouton, lui, ne l'est
                    pas — et il dit ce qu'il fait : continuer la réponse déjà
                    écrite, pas la recommencer. */}
                {t.erreurReprenable && t.question ? (
                  <div className="chat-reprise">
                    <button
                      className="chip"
                      onClick={() => reprendreLaMain(t.question as string)}
                      disabled={enCours}
                      title="Rouvre le flux avec cette réponse partielle : le modèle la continue au lieu de la recommencer."
                    >
                      reprendre la réponse
                    </button>
                  </div>
                ) : null}
              </div>
            )}
          </div>
        ))}
      </div>

      <div className="chat-pied">
        <textarea
          value={question}
          aria-label="Question au chat"
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); envoyer(); }
          }}
          placeholder={enCours ? "réponse en cours…" : "votre question (Entrée pour envoyer)"}
          rows={2}
        />
        <button className="chip on" onClick={envoyer} disabled={enCours || !question.trim()}>
          {enCours ? "…" : "envoyer"}
        </button>
      </div>
    </div>
  );
}
