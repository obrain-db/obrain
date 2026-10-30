/**
 * ── LA FLOTTE : CE QU'ON PEUT DIRE D'UNE MACHINE, ET CE QU'ON A LE DROIT DE
 *    LUI PROPOSER (logique PURE — ni React, ni réseau, ni DOM) ────────────────
 *
 * Le canal existe côté moteur depuis `crates/obrain-cli/src/remote.rs` : une
 * machine s'annonce, le moteur lui pousse des demandes, la machine rend un
 * verdict. Ce qui manquait n'était pas le canal mais SA LECTURE HUMAINE : la
 * liste des machines d'un compte, et le geste d'envoyer quelque chose à l'une
 * d'elles. Un canal qu'on ne peut pas regarder ne se distingue pas d'un canal
 * qui n'existe pas.
 *
 * POURQUOI LA LOGIQUE EST ICI ET NON DANS LE PANNEAU. Trois de ces règles
 * décident de ce que l'utilisateur CROIT — « cette machine est vivante », « ce
 * bouton est disponible », « c'est réussi » — et une règle qui décide d'une
 * croyance doit pouvoir être éprouvée sans navigateur. Le panneau (`Flotte.tsx`)
 * ne fait que rendre ce que ce module calcule ; `flotte.test.mjs` le vérifie sur
 * les valeurs RÉELLES du protocole, pas sur des valeurs inventées.
 *
 * ── LES TROIS VÉRITÉS QUE CE MODULE REFUSE DE FLOUER ────────────────────────
 *
 * 1. *Une machine dont on n'a pas de nouvelles n'est pas une machine vivante.*
 *    Le moteur ne rend qu'un entier — `silence_ms`, mesuré par LUI — et
 *    l'interface le traduisait volontiers en « connectée ». Ici, le silence est
 *    classé, et les seuils ne sont pas choisis au goût : ils viennent du
 *    protocole (voir `presenceDe`).
 *
 * 2. *On ne propose pas un outil qu'une machine n'annonce pas.*
 *    `attache.rs` n'annonce que ce que la politique locale autorise VRAIMENT
 *    (« afficher `exec` sur un nœud qui refuse d'exécuter ferait proposer à
 *    l'utilisateur une action vouée au refus »). Proposer davantage rendrait le
 *    refus systématique, donc invisible : exactement ce que la doctrine de ce
 *    dépôt appelle « ANNONCER EST DÉCIDER ».
 *
 * 3. *Un succès, un refus motivé et un délai dépassé ne sont pas trois nuances
 *    d'un même échec.* Ils viennent de trois endroits différents du protocole
 *    (le nœud, sa politique, la montre du moteur) et appellent trois gestes
 *    différents de l'utilisateur : continuer, corriger, ou rallumer la machine.
 *
 * ── CE QUI A ÉTÉ AJOUTÉ LE 2026-09-16 (même journée, second passage) ────────
 *
 * Le panneau consommait `/api/remote/clients` — une route qui liste, mais qui
 * ne RÉPOND PAS à la question de la flotte : elle ne dit ni combien de machines
 * sont connues, ni combien sont joignables, ni qu'il n'y en a aucune. Ces trois
 * silences-là se ressemblaient tous : « aucune machine » et « route absente »
 * rendaient le même écran vide. Le moteur a donc reçu une route qui RÉPOND
 * (`GET /api/remote/machines`, champs `compte`, `question`, `machines`,
 * `connues`, `joignables`, `aucune`, `reponse`), et ce module la LIT.
 *
 * Trois règles ont été ajoutées avec elle, et elles sont toutes éprouvées dans
 * `flotte.test.mjs` :
 *
 * 4. *Une liste vide n'est un fait que si le moteur l'AFFIRME.* `aucune` +
 *    `reponse` disent « ce compte ne connaît aucune machine ». Une liste vide
 *    SANS ces champs n'est pas la même chose : c'est un vide non affirmé, et
 *    l'écran doit le dire au lieu de le présenter comme une réponse (voir
 *    `etatDeLaListe`).
 *
 * 5. *« Vivante » est une mesure datée, pas une propriété permanente.* Le
 *    moteur mesure le silence AU MOMENT de la réponse ; l'affichage, lui, reste
 *    à l'écran. Un onglet en arrière-plan voit ses minuteries étranglées par le
 *    navigateur, et une liste lue il y a trois minutes continuerait d'annoncer
 *    « vivante ». `presenceDe(machine, ecoule_ms)` vieillit donc la mesure par
 *    une DURÉE mesurée localement — jamais par une soustraction de deux
 *    horloges différentes (voir le commentaire de la fonction).
 *
 * 6. *Ce qu'une machine ne fera pas doit être dit aussi clairement que ce
 *    qu'elle fera.* Afficher `read, list, status` laisse croire que le reste est
 *    une question de bouton ; `capacitesNonAnnoncees` nomme les outils ABSENTS
 *    et leur conséquence, pour que l'absence soit une information et non un
 *    blanc.
 */

// ═══════════════════════════════════════════════════════════════════════════════
// CE QUE LE MOTEUR REND — les champs EXACTS, recopiés de `Client::en_json`
// ═══════════════════════════════════════════════════════════════════════════════
/**
 * Une machine telle que le moteur la rend (`crates/obrain-cli/src/remote.rs`,
 * `Client::en_json`) :
 *
 *     { id, machine, capacites, attache_ms, derniere_vue_ms, silence_ms, vivant }
 *
 * AUCUN de ces champs n'est inventé, et l'interface n'en consomme aucun autre.
 * Le moteur ne rend PAS `compte` par machine (il l'a déjà filtré : la liste ne
 * contient que les machines de l'appelant) — le compte est rendu UNE fois, à la
 * racine de la réponse.
 *
 * POURQUOI UN SEUL TYPE POUR DEUX ROUTES. `GET /api/remote/clients` (route
 * héritée) et `GET /api/remote/machines` (route de la flotte) rendent le MÊME
 * objet par machine : c'est `Client::en_json` dans les deux cas. Ce qui diffère
 * est la RACINE de la réponse — `clients` + `aide` d'un côté, `machines` +
 * `connues`/`joignables`/`aucune`/`reponse`/`question` de l'autre. Dupliquer le
 * type par route ferait diverger deux lectures d'une même donnée ; c'est la
 * racine que `etatDeLaListe` sait lire dans les deux formes.
 */
export interface ClientFlotte {
  /** Identifiant de session de rattachement. Volontairement PAS un secret :
   *  ce qui autorise est le compte du jeton (décision 2 de `remote.rs`). */
  id: string;
  machine: string;
  /** Les capacités ANNONCÉES par le nœud — ce que sa politique lui permet. */
  capacites: string[];
  /** Date de rattachement (epoch ms, horloge du moteur). */
  attache_ms: number;
  /** Dernier signe de vie (epoch ms, horloge du moteur). */
  derniere_vue_ms: number;
  /** `now_ms() - derniere_vue_ms`, calculé PAR LE MOTEUR au moment de la réponse.
   *  C'est la seule mesure d'ancienneté utilisable : le navigateur et le moteur
   *  ne partagent pas forcément une horloge, et soustraire deux horloges
   *  différentes ne donne pas une durée. */
  silence_ms?: number;
  /**
   * ── « CONNUE N'EST PAS JOIGNABLE » — le champ que le registre persisté ajoute
   *
   * Le chantier du registre (journal append-only par compte, `remote.rs`) ajoute
   * ce champ avec un sens TRÈS précis, à ne pas confondre avec le silence :
   *
   *   `true`  — un battement a été vu par CE processus : la machine est
   *             réellement joignable ;
   *   `false` — la machine est CONNUE du journal, mais elle n'a parlé qu'à un
   *             processus MORT : le moteur REFUSERA de lui envoyer une demande.
   *
   * `clients_du_compte` le dit lui-même — « c'est à l'appelant de ne pas les
   * annoncer comme joignables, et le champ `vivant` est là pour ça ». Cet
   * appelant, c'est ce panneau.
   *
   * POURQUOI CE CHAMP EST FACULTATIF ICI : le 8432 sert aujourd'hui un binaire
   * construit AVANT ce champ. Le panneau doit donc être juste dans les deux
   * mondes — `false` interdit l'appel, `absent` laisse juger sur le silence
   * (comportement d'aujourd'hui). Un `undefined` traité comme `false`
   * effacerait la flotte d'un utilisateur dont le moteur est plus ancien ; un
   * `undefined` traité comme `true` annoncerait joignable ce qu'on ne sait pas.
   */
  vivant?: boolean;
}

/** La réponse entière de `GET /api/remote/clients` — la route HÉRITÉE. */
export interface ReponseClients {
  compte?: string;
  clients?: ClientFlotte[];
  /** Le texte que le moteur adresse à l'utilisateur : il nomme la commande
   *  exacte à lancer sur la machine (`selfd attacher …`). On l'affiche TEL QUEL
   *  quand la liste est vide : une liste vide sans mode d'emploi est un
   *  cul-de-sac. */
  aide?: string;
  error?: string;
}

/**
 * ── LA ROUTE QUI RÉPOND À LA QUESTION DE LA FLOTTE ─────────────────────────
 *
 * `GET /api/remote/machines` (handler `api_remote`, `viz_serve.rs`) : « quelles
 * machines connais-tu, et que sait faire chacune ? ». Champs recopiés du
 * `serde_json::json!` de la route, sans en inventer un seul :
 *
 *     { compte, question, machines[], connues, joignables, aucune, reponse }
 *
 * POURQUOI ELLE REMPLACE `clients` DANS CE PANNEAU. La route héritée liste des
 * clients SANS DIRE ce que la liste vaut : un compte sans machine et un moteur
 * qui n'a pas la route rendaient le même corps (`clients: []`). Ici l'absence
 * est un CHAMP (`aucune`) et une PHRASE (`reponse`) : « le vide n'a pas la forme
 * d'une bonne réponse » cesse d'être une intention et devient une donnée que
 * l'écran peut citer mot pour mot.
 *
 * `connues`/`joignables` sont des COMPTEURS DU MOTEUR, et c'est pour cela qu'ils
 * sont intéressants : confrontés à la liste qu'ils accompagnent, ils rendent une
 * incohérence VISIBLE (`accordCompteurs`) au lieu de la laisser à l'utilisateur
 * le soin de compter. Un compteur qu'on affiche sans le vérifier est une
 * affirmation qu'on n'a pas éprouvée.
 */
export interface ReponseMachines {
  compte?: string;
  /** La question posée, telle que le moteur la formule. On l'affiche : elle dit
   *  ce que la liste prétend répondre, et donc ce qu'elle ne couvre pas. */
  question?: string;
  machines?: ClientFlotte[];
  connues?: number;
  joignables?: number;
  /** `true` quand le moteur AFFIRME qu'il n'y a aucune machine — à distinguer
   *  d'une liste vide qu'il n'affirme pas. */
  aucune?: boolean;
  /** La phrase du moteur à l'utilisateur : elle nomme la commande exacte à
   *  lancer. Elle est citée TELLE QUELLE, jamais reformulée par l'interface. */
  reponse?: string;
  error?: string;
}

// ═══════════════════════════════════════════════════════════════════════════════
// LA PRÉSENCE — « depuis quand on l'a vue », et ce que cela permet d'affirmer
// ═══════════════════════════════════════════════════════════════════════════════
/**
 * Au-delà de ce silence, le MOTEUR OUBLIE le client (`SILENCE_MAX_MS` de
 * `remote.rs`, 90 s). Recopié ici pour une raison précise : une machine qui
 * apparaîtrait avec un silence supérieur à ce seuil ne serait pas « une machine
 * silencieuse », ce serait une LISTE PÉRIMÉE — le panneau n'a pas rafraîchi
 * depuis longtemps. Le panneau doit alors le dire, pas l'afficher comme un état
 * de la machine.
 */
export const SILENCE_OUBLI_MS = 90_000;

/**
 * La fenêtre du battement de cœur. Le client fait un long-poll de 20 s
 * (`ATTENTE_S` de `attache.rs`) et le moteur borne à 25 s
 * (`attente_max_s` de `api_remote`) ; chaque tour de poll REpose
 * `derniere_vue_ms`. Un silence qui atteint 30 s couvre donc un tour entier
 * perdu : la machine n'est plus « vivante » à proprement parler, elle est en
 * train de rater son battement.
 */
export const SEUIL_VIVANTE_MS = 30_000;

export type Presence = "vivante" | "froide" | "connue" | "muette" | "inconnue";

export interface EtatPresence {
  presence: Presence;
  /** Ce qu'on écrit à l'écran — jamais « connectée » quand on ne l'a pas vu. */
  libelle: string;
  /** Couleur du point : la distinction doit être VISIBLE, pas seulement lisible. */
  couleur: string;
  /** L'ancienneté retenue, en ms — `null` si le moteur n'en a pas donné.
   *  = `mesure_ms` + `ecoule_ms` : la mesure du moteur, vieillie par le temps
   *  écoulé DEPUIS la lecture. */
  age_ms: number | null;
  /** Ce que le MOTEUR a mesuré, au moment de sa réponse (`silence_ms`). */
  mesure_ms: number | null;
  /** Le temps écoulé depuis cette réponse, mesuré par le navigateur (durée,
   *  jamais une date : voir `presenceDe`). 0 quand l'appelant n'en fournit pas. */
  ecoule_ms: number;
}

/** Le silence retenu : la mesure du MOTEUR, ou `null` si elle manque. */
export function silenceDe(c: ClientFlotte): number | null {
  const s = c?.silence_ms;
  // `Number.isFinite` et non `!= null` : `NaN` traverse `!= null`, et un `NaN`
  // affiché comme âge donnerait « il y a NaN s ».
  if (typeof s === "number" && Number.isFinite(s) && s >= 0) return s;
  return null;
}

/**
 * ── UNE MESURE DATÉE DOIT VIEILLIR, SINON ELLE DEVIENT UNE AFFIRMATION ─────
 *
 * `silence_ms` est mesuré par le MOTEUR au moment où il répond. L'écran, lui,
 * reste affiché : le panneau relit toutes les 4 s, mais un onglet passé en
 * arrière-plan voit ses minuteries ÉTRANGLÉES par le navigateur (une minute et
 * plus entre deux tirs de `setInterval`) — et pendant tout ce temps la liste
 * continuerait d'afficher « vivante » sur une mesure vieille de plusieurs
 * minutes, c'est-à-dire d'affirmer une présence qu'elle n'a plus vérifiée.
 *
 * D'où ce second argument : `ecoule_ms`, le temps écoulé depuis la LECTURE.
 * L'arithmétique est précise et volontairement asymétrique :
 *
 *   `silence_ms` (durée, horloge du moteur) + `ecoule_ms` (DURÉE, mesurée par le
 *   navigateur entre deux instants de SA propre horloge) = une durée.
 *
 * On additionne deux DURÉES et on ne soustrait JAMAIS deux dates prises sur deux
 * horloges différentes — c'est cette soustraction-là qui produirait un âge faux
 * (et un « vivante » de complaisance) dès que la machine du navigateur et celle
 * du moteur ne sont pas réglées à la seconde près. Voir aussi le champ
 * `derniere_vue_ms`, qui est une DATE du moteur : elle s'AFFICHE, elle ne se
 * soustrait pas.
 *
 * Une valeur négative ou non finie est ramenée à 0 : une horloge locale qui
 * recule (mise à l'heure, machine en veille) ne doit pas rajeunir une machine.
 */
export function presencedAge(mesure: number | null, ecoule_ms: number): number | null {
  if (mesure === null) return null;
  const ecoule = typeof ecoule_ms === "number" && Number.isFinite(ecoule_ms) && ecoule_ms > 0 ? ecoule_ms : 0;
  return mesure + ecoule;
}

/**
 * Classe la présence d'une machine. Un silence inconnu n'est PAS « vivant » :
 * on ne comble pas une mesure absente par l'hypothèse la plus rassurante — c'est
 * ainsi qu'une interface se met à mentir par omission.
 *
 * L'ORDRE DES DEUX PREMIERS TESTS N'EST PAS INDIFFÉRENT. `vivant === false`
 * passe AVANT le silence, et c'est la correction qui compte : après un
 * redémarrage du moteur, une machine relue du journal peut afficher un silence
 * de quelques secondes — non parce qu'elle vient de parler, mais parce que le
 * moteur vient de redémarrer. Jugée sur ce seul silence, elle s'afficherait
 * « vivante » alors que personne n'a frappé à la porte de ce processus, et
 * l'utilisateur enverrait une demande vers une machine que le moteur refusera de
 * joindre. Le silence mesure le temps ; `vivant` dit QUI a entendu.
 *
 * `vivant === true` reste, lui aussi, plus fort que le vieillissement : c'est un
 * FAIT rapporté par le moteur (« un battement a été vu par ce processus »), et
 * aucune durée locale ne le contredit. Ce qui vieillit, c'est la PORTÉE de ce
 * fait : le libellé dit « à la dernière lecture » dès qu'on a vieilli la mesure,
 * pour qu'un « vivante » affiché ne se lise jamais comme un présent vérifié.
 */
export function presenceDe(c: ClientFlotte, ecoule_ms = 0): EtatPresence {
  const mesure = silenceDe(c);
  const ecoule = typeof ecoule_ms === "number" && Number.isFinite(ecoule_ms) && ecoule_ms > 0 ? ecoule_ms : 0;
  const age = presencedAge(mesure, ecoule);
  const depuis = ecoule > 0 ? " (à la dernière lecture)" : "";
  if (c?.vivant === false) {
    return {
      presence: "connue",
      libelle:
        age !== null
          ? `connue, pas joignable — se tait depuis ${depuisLisible(age)}`
          : "connue, pas joignable — aucun battement dans ce processus",
      couleur: "#b07aa1",
      age_ms: age,
      mesure_ms: mesure,
      ecoule_ms: ecoule,
    };
  }
  if (age === null) {
    return {
      presence: "inconnue",
      libelle: "présence non mesurée",
      couleur: "#8d9aab",
      age_ms: null,
      mesure_ms: null,
      ecoule_ms: ecoule,
    };
  }
  if (age <= SEUIL_VIVANTE_MS) {
    return {
      presence: "vivante",
      libelle: `vivante${depuis}`,
      couleur: "#6fd08c",
      age_ms: age,
      mesure_ms: mesure,
      ecoule_ms: ecoule,
    };
  }
  if (age <= SILENCE_OUBLI_MS) {
    return {
      presence: "froide",
      libelle: "sans nouvelles — battement manqué",
      couleur: "#e0b350",
      age_ms: age,
      mesure_ms: mesure,
      ecoule_ms: ecoule,
    };
  }
  return {
    presence: "muette",
    libelle: "muette — le moteur ne la garde plus (liste périmée)",
    couleur: "#ff7a5c",
    age_ms: age,
    mesure_ms: mesure,
    ecoule_ms: ecoule,
  };
}

/**
 * La machine peut-elle RÉELLEMENT recevoir une demande ?
 *
 * `false` seulement quand le moteur a dit `vivant: false` — un fait, pas une
 * déduction. Quand le champ est absent (binaire plus ancien), on ne prétend pas
 * savoir : on répond `true`, et c'est le moteur qui tranchera, avec un refus
 * motivé que le panneau affiche. Refuser localement sur une absence
 * d'information fermerait une porte que rien n'a fermée.
 */
export function joignable(c: ClientFlotte | null | undefined): boolean {
  return c?.vivant !== false;
}

/** Le motif à afficher quand l'appel est impossible — `null` s'il est possible. */
export function motifNonJoignable(c: ClientFlotte | null | undefined): string | null {
  if (!c || joignable(c)) return null;
  const age = silenceDe(c);
  return (
    `« ${c.machine} » est CONNUE du registre mais n'est PAS joignable : elle n'a parlé qu'à un ` +
    "processus moteur qui n'est plus là" +
    (age !== null ? ` (dernier battement il y a ${depuisLisible(age)})` : "") +
    ". Le moteur refuserait la demande — c'est le registre lui-même qui le dit (`vivant: false`). " +
    "Rallumez le nœud sur cette machine : elle redeviendra vivante en redemandant du travail."
  );
}

/**
 * Une durée en français lisible. Volontairement grossière : « il y a 4 s » se
 * lit, « il y a 3 812 ms » ne se lit pas. Les bornes sont celles qui changent la
 * DÉCISION de l'utilisateur (secondes → minutes → heures → jours).
 */
export function dureeLisible(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "durée inconnue";
  const s = Math.round(ms / 1000);
  if (s < 60) return `il y a ${s} s`;
  const min = Math.floor(s / 60);
  if (min < 60) return `il y a ${min} min`;
  const h = Math.floor(min / 60);
  if (h < 48) return `il y a ${h} h`;
  return `il y a ${Math.floor(h / 24)} j`;
}

/**
 * La même durée, mais du point de vue de la MACHINE : « depuis 12 min » et non
 * « il y a 12 min ». La nuance n'est pas cosmétique — « il y a 12 min » décrit un
 * événement vu par l'observateur, « depuis 12 min » décrit un état qui dure, et
 * c'est cet état qui décide si l'on peut encore parler à la machine.
 */
export function depuisLisible(ms: number): string {
  return dureeLisible(ms).replace(/^il y a /, "");
}

// ═══════════════════════════════════════════════════════════════════════════════
// LA LECTURE DE LA ROUTE — « la flotte répond », et quand elle ne répond pas
// ═══════════════════════════════════════════════════════════════════════════════
/** La route interrogée. `machines` = la route de la flotte (celle qui RÉPOND) ;
 *  `clients` = la route héritée, gardée pour les moteurs qui n'ont pas la
 *  première — elle liste, mais elle n'affirme rien. */
export type RouteFlotte = "machines" | "clients";

/**
 * Ce que la lecture a établi. Les six états ne sont pas six nuances d'échec :
 * chacun appelle un geste différent de l'utilisateur.
 *
 *  · `liste`           — des machines nommées : regarder, choisir, agir ;
 *  · `aucune`          — le moteur AFFIRME qu'il n'y en a aucune : attacher un
 *                        nœud (la phrase du moteur dit comment) ;
 *  · `vide_non_affirme`— liste vide SANS affirmation : c'est un DÉFAUT à
 *                        signaler, pas une réponse. Le confondre avec `aucune`
 *                        ferait chercher un nœud à attacher sur une installation
 *                        qui, peut-être, n'a jamais servi la flotte ;
 *  · `corps_illisible` — 200 et autre chose que la flotte (une page HTML, un
 *                        proxy, une autre route) : on ne décode pas au hasard ;
 *  · `route_absente`   — 404 : CE moteur ne connaît pas la route de la flotte.
 *                        À ne jamais présenter comme « aucune machine » ;
 *  · `identite_refusee`— 401/403 : le canal ne parle qu'à VOS machines, et il
 *                        refuse de dire lesquelles tant qu'il ne sait pas qui
 *                        demande. La liste affichée peut être périmée ;
 *  · `erreur_moteur`   — tout autre code : le motif du moteur est remonté tel
 *                        quel (un refus sans motif ressemble à une panne).
 */
export type EtatListe =
  | "liste"
  | "aucune"
  | "vide_non_affirme"
  | "corps_illisible"
  | "route_absente"
  | "identite_refusee"
  | "erreur_moteur";

/** Les compteurs du moteur confrontés à la liste qu'ils accompagnent. */
export type AccordCompteurs = "verifie" | "non_verifiable" | "desaccord";

export interface LectureFlotte {
  statut: number;
  route: RouteFlotte;
  /** Vrai si les machines viennent de la route HÉRITÉE (`clients`) : l'écran
   *  doit le dire, parce que ce chemin ne rend ni `aucune` ni `joignables`. */
  heritee: boolean;
  etat: EtatListe;
  machines: ClientFlotte[];
  /** Nombre d'entrées de `machines` écartées parce qu'elles n'étaient pas des
   *  objets : une entrée illisible est comptée et dite, pas ignorée. */
  ecartees: number;
  compte: string | null;
  question: string | null;
  /** La phrase du moteur (`reponse` de la route de la flotte, `aide` de la route
   *  héritée) — citée TELLE QUELLE. */
  reponse: string | null;
  aucune: boolean | null;
  /** Ce que le MOTEUR déclare (null quand il ne le déclare pas). */
  compteurs: { connues: number | null; joignables: number | null };
  /** Ce que l'INTERFACE a compté sur la liste reçue. `joignables` vaut null si
   *  au moins une machine ne porte pas `vivant` : on ne compte pas ce qu'on ne
   *  peut pas distinguer. */
  comptees: { total: number; joignables: number | null };
  accordCompteurs: AccordCompteurs;
  /** Pourquoi les compteurs ne s'accordent pas (ou ne sont pas vérifiables). */
  motifCompteurs: string | null;
  /** La phrase de l'interface à l'écran — jamais un blanc. */
  message: string;
  /** Le motif du moteur, remonté sans reformulation (401, 404, 5xx). */
  motif: string | null;
  /** Le corps reçu, borné : la seule pièce recopiable dans un rapport. */
  brut: string;
}

/** Lit un entier de JSON sans le confondre avec autre chose. */
function entierDe(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

/** Lit un texte non vide de JSON (`null` sinon : un `""` n'est pas une phrase). */
function texteDe(v: unknown): string | null {
  return typeof v === "string" && v.trim() ? v : null;
}

/**
 * Traduit la réponse HTTP de la route en lecture — et l'absence en INFORMATION.
 *
 * C'est ici que se joue la règle la plus facile à enfreindre : *une réponse vide
 * ne doit jamais avoir la forme d'une bonne réponse*. Une liste vide n'est
 * affichée comme un fait (« aucune machine ») que si le moteur l'AFFIRME par
 * `aucune: true` ET le dit par `reponse`. Dans tous les autres cas — champ
 * `aucune` absent, `reponse` muette, route inconnue, corps qui n'est pas la
 * flotte — l'écran reçoit un ÉTAT DISTINCT, donc un affichage distinct.
 *
 * Les compteurs `connues`/`joignables` sont confrontés à la liste : un compteur
 * affiché sans vérification serait une affirmation de plus, non éprouvée.
 */
export function etatDeLaListe(statut: number, corps: string, route: RouteFlotte = "machines"): LectureFlotte {
  const brut = String(corps ?? "").slice(0, 4000);
  const heritee = route === "clients";
  const nomChamp = heritee ? "clients" : "machines";
  let d: any = null;
  try {
    d = brut ? JSON.parse(brut) : null;
  } catch {
    d = null;
  }
  const motif = texteDe(d?.error) || texteDe(d?.message) || (brut ? brut.slice(0, 300) : "réponse vide");
  const base = {
    statut,
    route,
    heritee,
    machines: [] as ClientFlotte[],
    ecartees: 0,
    compte: texteDe(d?.compte),
    question: texteDe(d?.question),
    reponse: texteDe(d?.reponse) || texteDe(d?.aide),
    aucune: typeof d?.aucune === "boolean" ? (d.aucune as boolean) : null,
    compteurs: { connues: entierDe(d?.connues), joignables: entierDe(d?.joignables) },
    comptees: { total: 0, joignables: null as number | null },
    accordCompteurs: "non_verifiable" as AccordCompteurs,
    motifCompteurs: null as string | null,
    motif: null as string | null,
    brut,
  };
  const rendre = (etat: EtatListe, message: string, extra: Partial<LectureFlotte> = {}): LectureFlotte => ({
    ...base,
    etat,
    message,
    ...extra,
  });

  // ── LE REFUS D'IDENTITÉ PASSE AVANT TOUT DÉCODAGE ─────────────────────────
  // Il ne dit rien des machines, et c'est justement pourquoi il ne doit pas
  // produire un écran vide : un écran vide se lirait « aucune machine ».
  if (statut === 401 || statut === 403) {
    return rendre(
      "identite_refusee",
      `le moteur refuse l'identité (HTTP ${statut}) : ${motif} — le canal ne parle qu'à VOS ` +
        "machines, et il ne dit pas lesquelles tant qu'il ne sait pas qui demande. " +
        "Reconnectez-vous : la liste affichée peut être périmée.",
      { motif },
    );
  }

  // ── 404 : CE MOTEUR N'A PAS LA ROUTE ─────────────────────────────────────
  // C'est le cas que « ANNONCER EST DÉCIDER » interdit de maquiller en silence :
  // « route absente » et « aucune machine » ne peuvent pas rendre le même écran.
  if (statut === 404) {
    return rendre(
      "route_absente",
      `ce moteur ne connaît pas GET /api/remote/${route} (HTTP 404 : ${motif}). ` +
        (heritee
          ? "Aucune des deux routes de la flotte ne répond : la liste des machines ne peut pas être lue ici."
          : "Le binaire servi est antérieur à la route de la flotte. La liste est donc relue par la route héritée " +
            "(`/api/remote/clients`, qui liste mais n'affirme ni le vide ni les compteurs) — et il faut le dire, " +
            "car ce chemin ne saura jamais dire « aucune machine »."),
      { motif },
    );
  }

  if (statut !== 200) {
    return rendre(
      "erreur_moteur",
      `le moteur a répondu HTTP ${statut} : ${motif}. Rien n'est affirmé sur vos machines — ` +
        "un refus doit être MOTIVÉ et remonté, c'est pourquoi il est affiché ici tel quel.",
      { motif },
    );
  }

  // ── 200 : LE CORPS EST-IL SEULEMENT LA FLOTTE ? ──────────────────────────
  // Un 200 ne prouve rien : un mandataire, une page d'accueil ou une autre route
  // répondent aussi 200. On exige donc le champ de liste, et on refuse de
  // deviner (`corps_illisible` n'est pas « aucune machine »).
  if (!d || typeof d !== "object" || Array.isArray(d)) {
    return rendre(
      "corps_illisible",
      `le moteur a répondu 200 avec un corps qui n'est PAS la flotte (il n'est pas même un objet JSON). ` +
        "C'est le fait d'un mandataire, d'une page servie à la place de l'API, ou d'une route détournée — " +
        "et non l'absence de machines. Corps reçu ci-dessous.",
    );
  }
  if (!Array.isArray(d[nomChamp])) {
    return rendre(
      "corps_illisible",
      `le moteur a répondu 200 SANS le champ « ${nomChamp} » : la réponse est bien du JSON, mais ce n'est pas ` +
        "la flotte. L'interface refuse de deviner ce qu'elle n'a pas reçu — ce n'est pas « aucune machine ».",
    );
  }
  const brutes: unknown[] = d[nomChamp];
  const machines = brutes.filter(
    (m) => m !== null && typeof m === "object" && !Array.isArray(m),
  ) as ClientFlotte[];
  const ecartees = brutes.length - machines.length;
  if (brutes.length > 0 && machines.length === 0) {
    return rendre(
      "corps_illisible",
      `le moteur a rendu ${brutes.length} entrée(s) dans « ${nomChamp} », dont AUCUNE n'est une machine ` +
        "(objet nommé) : rien ne peut être affiché sans inventer. Ce n'est pas « aucune machine ».",
      { ecartees },
    );
  }

  // ── LES COMPTEURS, CONFRONTÉS À LA LISTE ─────────────────────────────────
  const toutesVivant = machines.every((m) => typeof m?.vivant === "boolean");
  const comptees = {
    total: machines.length,
    joignables: toutesVivant ? machines.filter((m) => m.vivant === true).length : null,
  };
  const desaccords: string[] = [];
  const nonVerifiables: string[] = [];
  if (ecartees > 0) {
    desaccords.push(`${ecartees} entrée(s) de « ${nomChamp} » n'étaient pas des machines et ont été écartées`);
  }
  if (base.compteurs.connues !== null && base.compteurs.connues !== machines.length) {
    desaccords.push(
      `le moteur déclare ${base.compteurs.connues} machine(s) connue(s) mais en nomme ${machines.length}`,
    );
  }
  if (base.compteurs.joignables !== null) {
    if (!toutesVivant) {
      nonVerifiables.push(
        `le moteur déclare ${base.compteurs.joignables} joignable(s) sans que chaque machine porte « vivant » : ` +
          "le compte n'est pas vérifiable par l'interface (binaire antérieur ?)",
      );
    } else if (base.compteurs.joignables !== comptees.joignables) {
      desaccords.push(
        `le moteur déclare ${base.compteurs.joignables} joignable(s) mais ${comptees.joignables} machine(s) ` +
          "seulement portent « vivant: true »",
      );
    }
  }
  const motifCompteurs = [...desaccords, ...nonVerifiables].join(" ; ") || null;
  const accordCompteurs: AccordCompteurs = desaccords.length
    ? "desaccord"
    : nonVerifiables.length || base.compteurs.connues === null
      ? "non_verifiable"
      : "verifie";
  const communs = {
    machines,
    ecartees,
    comptees,
    accordCompteurs,
    motifCompteurs:
      motifCompteurs ||
      (base.compteurs.connues === null
        ? "le moteur ne rend aucun compteur : ce chemin (route héritée) ne dit ni combien de machines il connaît, ni combien sont joignables."
        : null),
  };

  // ── LA LISTE VIDE : UN FAIT SEULEMENT SI LE MOTEUR L'AFFIRME ─────────────
  if (machines.length === 0) {
    const phrase = base.reponse;
    if (base.aucune === true && phrase) {
      // LA forme exigée : le moteur affirme (`aucune`) ET dit (`reponse`). On
      // cite sa phrase — elle nomme la commande exacte à lancer, donc elle ne
      // peut pas se périmer dans une page.
      return rendre("aucune", phrase, communs);
    }
    if (base.aucune === true) {
      return rendre(
        "aucune",
        "ce compte ne connaît AUCUNE machine (le moteur l'affirme par `aucune`) — mais il n'a pas joint de " +
          "phrase d'accompagnement (`reponse`) : le mode d'emploi manque, et un vide sans mode d'emploi est un cul-de-sac.",
        communs,
      );
    }
    return rendre(
      "vide_non_affirme",
      "le moteur a rendu une liste de machines VIDE, mais il ne l'AFFIRME pas" +
        (base.aucune === false
          ? " (et il dit même `aucune: false`)"
          : heritee
            ? " — la route héritée ne porte pas de champ `aucune` : elle ne sait pas affirmer une absence"
            : " — les champs `aucune`/`reponse` manquent") +
        ". Une liste vide sans affirmation n'est pas une réponse : l'absence de machine et l'absence de route " +
        "restent indistinguables, et c'est exactement ce que la route de la flotte a été écrite pour lever." +
        (phrase ? ` Phrase du moteur : « ${phrase} »` : ""),
      communs,
    );
  }

  return rendre(
    "liste",
    base.reponse ||
      `${machines.length} machine(s) connue(s) de ce compte` +
        (comptees.joignables !== null ? `, ${comptees.joignables} joignable(s) à la lecture` : ""),
    communs,
  );
}

/**
 * Faut-il redemander la liste à la route HÉRITÉE ?
 *
 * Seulement sur un 404 : c'est le seul état où l'on SAIT que la route de la
 * flotte n'existe pas ici. Sur tout autre état (401, 5xx, corps illisible), la
 * route existe et elle a parlé — la relire ailleurs ne ferait que masquer ce
 * qu'elle a dit. Et la bascule n'est jamais silencieuse : `heritee` et
 * `motifCompteurs` portent la raison jusqu'à l'écran.
 */
export function doitBasculerSurRouteHeritee(l: LectureFlotte | null | undefined): boolean {
  return l?.etat === "route_absente" && !l.heritee;
}

/**
 * ── QUELLE LISTE MONTRER, ET EN LE DISANT ──────────────────────────────────
 *
 * Quand la lecture ÉCHOUE (identité refusée, moteur en erreur, corps illisible,
 * route absente des deux côtés), effacer la liste serait une perte sèche : le
 * refus dirait « je ne sais plus rien de vos machines » alors que la dernière
 * lecture, elle, savait. On garde donc la dernière liste CONNUE — mais jamais
 * comme si elle était fraîche : `perimee: true` et sa DATE (`le`) remontent
 * jusqu'à l'écran, pour que l'utilisateur sache sur quoi il regarde. Et `le` est
 * la date de la LECTURE QUI A RENDU CETTE LISTE, pas celle de la dernière
 * tentative : c'est elle qui sert à vieillir les présences.
 *
 * L'inverse est vrai aussi, et c'est la règle qui compte : une lecture RÉUSSIE
 * remplace la liste, y compris quand elle est VIDE. Le cache ne doit jamais
 * contredire une réponse fraîche — un cache qu'on préfère à la réponse s'appelle
 * un mensonge.
 *
 * `vide_non_affirme` (200 + liste vide sans affirmation) remplace donc la liste
 * par du vide : la lecture a parlé, et ce qu'elle a dit est qu'elle n'affirme
 * rien. Montrer quand même l'ancienne liste ferait passer un défaut de route
 * pour un état de la flotte.
 */
export function listeAffichable(
  lecture: LectureFlotte | null | undefined,
  derniere: { machines: ClientFlotte[]; le: number } | null | undefined,
  leLecture: number | null = null,
  transportEnEchec = false,
): { machines: ClientFlotte[]; perimee: boolean; le: number | null; motif: string | null } {
  const affichage =
    lecture && (lecture.etat === "liste" || lecture.etat === "aucune" || lecture.etat === "vide_non_affirme")
      ? { machines: lecture.machines, perimee: false, le: leLecture, motif: null as string | null }
      : derniere && derniere.machines.length
        ? {
            machines: derniere.machines,
            perimee: true,
            le: derniere.le,
            motif:
              "la lecture actuelle n'a pas abouti (voir le motif ci-dessus) : cette liste est la DERNIÈRE CONNUE, " +
              "et rien ne dit que ces machines sont encore attachées.",
          }
        : { machines: [] as ClientFlotte[], perimee: false, le: null, motif: null as string | null };

  // ── LE TRANSPORT A LÂCHÉ : MÊME UNE LECTURE « RÉUSSIE » EST VIEILLE ────────
  // `etatDeLaListe` ne voit que des réponses HTTP ; une exception réseau ne lui
  // en donne aucune, et la dernière lecture RÉUSSIE reste alors à l'écran. Elle
  // est juste — mais elle n'est plus vérifiée, et la présenter comme fraîche
  // ferait dire « vivante » à une machine dont on n'a plus de nouvelles depuis
  // l'échec. Le drapeau est donc passé par l'appelant, qui seul sait que la
  // relecture n'est pas revenue.
  if (transportEnEchec && affichage.machines.length && !affichage.perimee) {
    return {
      ...affichage,
      perimee: true,
      motif:
        "le moteur n'a pas répondu à la dernière relecture (voir le motif ci-dessus) : cette liste est la " +
        "DERNIÈRE CONNUE, et rien ne dit que ces machines sont encore attachées.",
    };
  }
  return affichage;
}

// ═══════════════════════════════════════════════════════════════════════════════
// LES CAPACITÉS — « on ne propose que ce qui est annoncé »
// ═══════════════════════════════════════════════════════════════════════════════
/**
 * Les capacités annoncées, dédoublonnées, dans l'ordre ANNONCÉ par la machine.
 * L'ordre est conservé à dessein : il porte la priorité de celui qui sait ce
 * qu'il peut faire (`capacites()` de `attache.rs` met `read`, `list`, `write`,
 * `status` puis `exec` en dernier — l'outil le plus dangereux en dernier).
 */
export function capacitesAnnoncees(c: ClientFlotte | null | undefined): string[] {
  const vues = new Set<string>();
  const out: string[] = [];
  for (const brut of c?.capacites ?? []) {
    const n = String(brut ?? "").trim();
    if (!n || vues.has(n)) continue;
    vues.add(n);
    out.push(n);
  }
  return out;
}

/** Une entrée du formulaire d'arguments d'un outil. */
export interface ChampOutil {
  nom: string;
  type: "texte" | "entier" | "booleen" | "liste";
  requis: boolean;
  aide?: string;
}

/** Fiche d'un outil annoncé : ce qu'il fait, et ce qu'il attend. */
export interface FicheOutil {
  resume: string;
  champs: ChampOutil[];
}

/**
 * ── LES FORMES D'ARGUMENTS, RECOPIÉES DU NŒUD, PAS DEVINÉES ─────────────────
 *
 * Source : `crates/obrain-selfd/src/agent.rs` (`outils_modele()`, le schéma que
 * le nœud présente LUI-MÊME à son modèle) et `outils.rs` pour la lecture des
 * champs. Les noms sont ceux du protocole : `path`, `content`, `cmd`, `args`,
 * `cwd`, `shell`, `maxBytes`.
 *
 * POURQUOI UNE FICHE PLUTÔT QU'UN CHAMP JSON LIBRE PARTOUT : un formulaire qui
 * n'aide pas laisse l'utilisateur deviner, et un argument deviné fait rendre au
 * nœud un refus dont la cause est l'interface, pas la politique. Un outil annoncé
 * qui n'aurait PAS de fiche tombe sur le champ JSON libre (voir `champsDe`) : on
 * n'enferme jamais une machine dans le catalogue d'une version de l'interface.
 */
export const FICHE_OUTILS: Record<string, FicheOutil> = {
  read: {
    resume: "Lire un fichier de la machine (chemin sous la racine autorisée)",
    champs: [
      { nom: "path", type: "texte", requis: true, aide: "ex. notes/journal.md" },
      { nom: "maxBytes", type: "entier", requis: false, aide: "borne de lecture" },
    ],
  },
  list: {
    resume: "Lister un répertoire de la machine",
    champs: [{ nom: "path", type: "texte", requis: false, aide: "défaut : « . », sous la racine" }],
  },
  write: {
    resume: "Écrire un fichier de la machine (chemin sous la racine autorisée)",
    champs: [
      { nom: "path", type: "texte", requis: true, aide: "ex. notes/constat.md" },
      { nom: "content", type: "texte", requis: true, aide: "contenu complet du fichier" },
    ],
  },
  exec: {
    resume: "Exécuter un programme sur la machine — refusé si le nœud n'a pas --allow-exec",
    champs: [
      { nom: "cmd", type: "texte", requis: true, aide: "le PROGRAMME (ex. df), pas une ligne de shell" },
      { nom: "args", type: "liste", requis: false, aide: "un argument par ligne" },
      { nom: "cwd", type: "texte", requis: false, aide: "répertoire, sous la racine du nœud" },
      { nom: "shell", type: "booleen", requis: false, aide: "passer par /bin/sh -c (à demander explicitement)" },
    ],
  },
  status: {
    resume: "L'état du nœud : identité, politique, racine, mode, depuis quand il tourne",
    champs: [],
  },
};

/** La fiche d'un outil, ou `null` si cette version de l'interface ne la connaît
 *  pas — l'appelant bascule alors sur le champ JSON libre. */
export function ficheDe(outil: string): FicheOutil | null {
  return FICHE_OUTILS[outil] ?? null;
}

/**
 * ── CE QU'UNE MACHINE NE FERA PAS ───────────────────────────────────────────
 *
 * Défaut comblé le 2026-09-16 : le panneau affichait les capacités PRÉSENTES
 * (`read`, `list`, `status`) et laissait le reste dans un blanc. Un blanc ne se
 * lit pas « cette machine n'exécute rien » — il se lit « je n'ai pas encore
 * trouvé le bouton ». Et « ANNONCER EST DÉCIDER » vaut dans les deux sens : si
 * une machine n'annonce pas `exec`, l'interface doit le DIRE, parce que c'est
 * une information sur ce que la machine EST (sa politique locale, fixée à son
 * démarrage), pas un détail d'ergonomie.
 *
 * Chaque absence est donc nommée AVEC sa conséquence, tirée du catalogue des
 * outils que le nœud présente lui-même à son modèle (`outils_modele()` de
 * `agent.rs`, recopié dans `FICHE_OUTILS`). Une absence sans conséquence serait
 * une ligne de plus à lire ; une conséquence sans absence serait une menace.
 */
export interface CapaciteAbsente {
  outil: string;
  /** Ce que l'absence EMPÊCHE, dit du point de vue de la machine. */
  consequence: string;
}

/** Les conséquences, par outil. Aucune n'est devinée : elles disent au négatif
 *  ce que l'outil fait (voir `FICHE_OUTILS`). */
const CONSEQUENCE_ABSENCE: Record<string, string> = {
  read: "elle ne lira AUCUN fichier",
  list: "elle ne listera AUCUN répertoire",
  write: "elle n'écrira AUCUN fichier — rien ne peut y être déposé depuis cette interface",
  exec: "elle n'exécutera AUCUN programme (son nœud n'a pas été démarré avec « --allow-exec »)",
  status: "elle ne dira pas son propre état (identité, politique, racine, mode, depuis quand elle tourne)",
};

/**
 * Les outils du catalogue que cette machine n'annonce PAS, dans l'ordre du
 * catalogue (le même que `FICHE_OUTILS`, donc celui du nœud : le plus dangereux
 * en dernier).
 *
 * Une machine absente (`null`/`undefined`) ne rend AUCUNE absence : on ne
 * reproche pas à une machine qui n'existe pas ce qu'elle ne fait pas.
 */
export function capacitesNonAnnoncees(c: ClientFlotte | null | undefined): CapaciteAbsente[] {
  if (!c) return [];
  const annoncees = capacitesAnnoncees(c);
  const absentes: CapaciteAbsente[] = [];
  for (const outil of Object.keys(FICHE_OUTILS)) {
    if (annoncees.includes(outil)) continue;
    absentes.push({
      outil,
      consequence: CONSEQUENCE_ABSENCE[outil] ?? `« ${outil} » ne lui sera pas proposé`,
    });
  }
  return absentes;
}

/**
 * Les capacités annoncées que CETTE version de l'interface ne connaît pas.
 *
 * Ce n'est pas une anomalie : le nœud peut annoncer un outil plus récent que la
 * page (c'est la raison d'être du champ JSON libre). Elles sont listées à part
 * pour qu'on ne les confonde pas avec les outils dont l'interface connaît la
 * fiche — la différence entre « je sais vous guider » et « je vous laisse
 * écrire les arguments ».
 */
export function capacitesHorsCatalogue(c: ClientFlotte | null | undefined): string[] {
  return capacitesAnnoncees(c).filter((n) => !(n in FICHE_OUTILS));
}

/**
 * La phrase des absences, prête à afficher — `null` quand la machine annonce
 * tout ce que l'interface sait proposer : il n'y a alors rien à dire, et une
 * ligne « aucune absence » serait du bruit.
 */
export function absencesLisible(c: ClientFlotte | null | undefined): string | null {
  if (!c) return null;
  const manques = capacitesNonAnnoncees(c);
  if (!manques.length) return null;
  const annoncees = capacitesAnnoncees(c);
  const tete = annoncees.length
    ? `Cette machine n'annonce PAS : ${manques.map((m) => m.outil).join(", ")} — `
    : "Cette machine n'annonce AUCUNE capacité — ";
  return (
    tete +
    manques.map((m) => m.consequence).join(" ; ") +
    ". L'interface ne propose jamais un outil qu'une machine n'annonce pas."
  );
}

// ═══════════════════════════════════════════════════════════════════════════════
// LE CONTRÔLE AVANT ENVOI — ce que l'interface refuse de laisser partir
// ═══════════════════════════════════════════════════════════════════════════════
export type VerifAppel =
  | { ok: true; arguments: Record<string, unknown> }
  | { ok: false; motif: string };

/**
 * Vérifie qu'un appel peut partir, AVANT de consommer un aller-retour réseau.
 *
 * Les trois refus ci-dessous sont ceux que l'interface peut opposer elle-même ;
 * tout le reste — la politique du nœud, la racine autorisée, le délai — ne peut
 * être jugé que par le nœud ou le moteur, et leurs refus REMONTENT (voir
 * `lireVerdict`). L'interface ne préjuge jamais d'un refus qu'elle ne peut pas
 * établir : elle n'en invente pas, et n'en cache aucun.
 */
export function verifierAppel(
  machine: ClientFlotte | null | undefined,
  outil: string,
  argsTexte: string,
): VerifAppel {
  if (!machine) {
    return { ok: false, motif: "aucune machine choisie : la demande ne serait adressée à personne." };
  }
  const nom = String(outil ?? "").trim();
  if (!nom) {
    return { ok: false, motif: "« outil » requis : que doit faire la machine ?" };
  }
  const annoncees = capacitesAnnoncees(machine);
  if (!annoncees.includes(nom)) {
    return {
      ok: false,
      motif:
        `« ${nom} » n'est pas annoncé par « ${machine.machine} » — ` +
        (annoncees.length
          ? `elle annonce : ${annoncees.join(", ")}. `
          : "elle n'annonce AUCUNE capacité. ") +
        "L'interface ne propose jamais un outil qu'une machine n'annonce pas.",
    };
  }
  const texte = String(argsTexte ?? "").trim();
  if (!texte) return { ok: true, arguments: {} };
  let v: unknown;
  try {
    v = JSON.parse(texte);
  } catch (e) {
    return { ok: false, motif: `arguments illisibles (JSON attendu) : ${String((e as Error)?.message || e)}` };
  }
  if (v === null || typeof v !== "object" || Array.isArray(v)) {
    return { ok: false, motif: "les arguments doivent être un OBJET JSON, par exemple {\"path\":\"notes.md\"}" };
  }
  return { ok: true, arguments: v as Record<string, unknown> };
}

/**
 * Construit les arguments depuis les valeurs saisies dans la fiche de l'outil.
 *
 * Le typage est fait ICI parce que le moteur transmet `arguments` TEL QUEL au
 * nœud : `read {"path": 42}` serait refusé par le nœud pour une raison qui
 * n'apprend rien à l'utilisateur (« path » n'est pas une chaîne), alors que la
 * cause est une case mal remplie dans l'interface.
 *
 * Un champ non requis laissé VIDE n'est pas envoyé du tout : envoyer
 * `{"path": ""}` sur `list` ferait perdre au nœud son défaut (« . »), c'est-à-dire
 * changerait le sens de la demande par rapport à ce que l'utilisateur a écrit.
 */
export function construireArguments(
  outil: string,
  valeurs: Record<string, string>,
): VerifAppel {
  const fiche = ficheDe(outil);
  if (!fiche) return { ok: false, motif: `pas de fiche d'arguments pour « ${outil} »` };
  const args: Record<string, unknown> = {};
  for (const ch of fiche.champs) {
    const brut = String(valeurs?.[ch.nom] ?? "").trim();
    if (!brut) {
      if (ch.requis) {
        return { ok: false, motif: `« ${ch.nom} » est requis par l'outil ${outil}.` };
      }
      continue;
    }
    switch (ch.type) {
      case "entier": {
        const n = Number(brut);
        if (!Number.isFinite(n)) return { ok: false, motif: `« ${ch.nom} » doit être un nombre entier.` };
        args[ch.nom] = Math.trunc(n);
        break;
      }
      case "booleen":
        args[ch.nom] = brut === "true";
        break;
      case "liste":
        args[ch.nom] = brut.split("\n").map((x) => x.trim()).filter(Boolean);
        break;
      default:
        args[ch.nom] = brut;
    }
  }
  return { ok: true, arguments: args };
}

// ═══════════════════════════════════════════════════════════════════════════════
// LE VERDICT — trois affichages DISTINCTS, et jamais un silence
// ═══════════════════════════════════════════════════════════════════════════════
export type GenreVerdict =
  | "succes"
  | "refus_machine"
  | "delai"
  | "refus_moteur"
  | "refus_interface"
  | "verdict_brut"
  | "transport";

export interface Verdict {
  genre: GenreVerdict;
  titre: string;
  detail: string;
  couleur: string;
  /** La charge utile rendue par la machine, quand il y en a une. */
  resultat?: unknown;
  /** Le corps brut de la réponse : on ne le jette JAMAIS — c'est la seule
   *  pièce qu'un utilisateur puisse recopier dans un rapport de défaut. */
  brut?: string;
}

/**
 * Traduit la réponse de `POST /api/remote/appel` en verdict lisible.
 *
 * ── LES TROIS SOURCES, ET POURQUOI ELLES NE SE CONFONDENT PAS ───────────────
 *
 * · **Le NŒUD a répondu** → HTTP 200, corps `{"resultat": …}`. `attache.rs`
 *   (`executer`) n'échoue JAMAIS : un succès, une erreur d'outil et un refus de
 *   politique sont trois RÉSULTATS, et deux formes seulement :
 *       succès  : { "ok": true,  "resultat": <valeur> }
 *       refus   : { "ok": false, "refus": "<motif>" }
 *   Le motif du refus est ce qui rend l'échec ACTIONNABLE — on l'affiche, on ne
 *   le résume pas.
 *
 * · **Le MOTEUR a renoncé** → HTTP 409, corps `{"error": "<motif>"}`. Deux
 *   causes très différentes y arrivent, et les confondre serait la faute :
 *   `remote::appeler` rend « le client n'a pas répondu en N s » quand la montre
 *   a expiré (le nœud est peut-être en train de TRAVAILLER), et « aucun client de
 *   ce nom n'est connecté » ou « ce client n'est pas à vous » quand il n'y avait
 *   personne (le nœud n'a rien reçu). Un délai invite à rallumer la machine ; un
 *   « personne » invite à la rattacher. Le motif exact est testé, pas deviné.
 *
 * · **Le transport a échoué** → ni 200 ni 409 : le verdict est INCONNU. On le
 *   dit, et on ne le présente ni comme un succès ni comme un refus.
 */
export function lireVerdict(statut: number, corps: string): Verdict {
  const brut = String(corps ?? "").slice(0, 4000);
  let d: any = null;
  try {
    d = brut ? JSON.parse(brut) : null;
  } catch {
    d = null;
  }
  const messageServeur =
    (typeof d?.error === "string" && d.error) ||
    (typeof d?.message === "string" && d.message) ||
    (brut ? brut.slice(0, 300) : "réponse vide");

  if (statut === 401 || statut === 403) {
    return {
      genre: "refus_moteur",
      titre: "Identité refusée par le moteur",
      detail: `${messageServeur} — reconnectez-vous : le canal ne parle qu'à VOS machines (jeton ou cookie de session requis).`,
      couleur: "#ff7a5c",
      brut,
    };
  }

  if (statut === 409) {
    // Le délai se reconnaît à la phrase du moteur, recopiée de `remote::appeler` :
    // « le client n'a pas répondu en {n} s ». On teste le fragment stable.
    if (/n'a pas répondu en\s*\d+\s*s/i.test(messageServeur)) {
      return {
        genre: "delai",
        titre: "Délai dépassé — la machine n'a pas rendu de verdict",
        detail:
          `${messageServeur}. Le moteur a retiré sa demande : elle ne sera pas exécutée plus tard. ` +
          "La machine est peut-être éteinte, en veille, ou son battement est interrompu — " +
          "regardez sa présence ci-dessus avant de réessayer.",
        couleur: "#e0b350",
        brut,
      };
    }
    return {
      genre: "refus_moteur",
      titre: "Le moteur a refusé d'émettre la demande",
      detail: messageServeur,
      couleur: "#ff7a5c",
      brut,
    };
  }

  if (statut === 400) {
    return {
      genre: "refus_moteur",
      titre: "Demande incomplète — refusée par le moteur",
      detail: messageServeur,
      couleur: "#ff7a5c",
      brut,
    };
  }

  if (statut === 200) {
    if (!d || typeof d !== "object" || !("resultat" in d)) {
      return {
        genre: "verdict_brut",
        titre: "Verdict rendu, forme non reconnue",
        detail:
          "Le moteur a répondu 200 sans champ `resultat` : rien n'est affirmé sur le succès. Corps brut ci-dessous.",
        couleur: "#8d9aab",
        resultat: d,
        brut,
      };
    }
    const r = (d as any).resultat;
    if (r && typeof r === "object" && !Array.isArray(r) && typeof (r as any).ok === "boolean") {
      const o = r as any;
      if (o.ok === true) {
        return {
          genre: "succes",
          titre: "Succès — la machine a exécuté la demande",
          detail: "Le nœud a rendu un verdict positif (`ok: true`).",
          couleur: "#6fd08c",
          resultat: o.resultat,
          brut,
        };
      }
      const motif =
        (typeof o.refus === "string" && o.refus) ||
        (typeof o.erreur === "string" && o.erreur) ||
        (typeof o.error === "string" && o.error) ||
        "refus sans motif";
      return {
        genre: "refus_machine",
        titre: "Refus de la machine — motivé",
        detail:
          `${motif} — c'est la politique LOCALE du nœud qui a tranché, pas le moteur : ` +
          "elle est fixée à son démarrage et aucun appel ne peut la desserrer.",
        couleur: "#e07a9a",
        resultat: o,
        brut,
      };
    }
    return {
      genre: "verdict_brut",
      titre: "Verdict rendu (forme non standard)",
      detail:
        "La machine a répondu, mais sans les champs `ok`/`refus` du protocole de `attache.rs` : " +
        "aucun succès n'est affirmé ici. Charge utile affichée telle quelle.",
      couleur: "#8d9aab",
      resultat: r,
      brut,
    };
  }

  return {
    genre: "transport",
    titre: `Réponse inattendue du moteur (HTTP ${statut})`,
    detail: `${messageServeur} — le verdict de la machine est INCONNU : ni succès, ni refus établis.`,
    couleur: "#ff7a5c",
    brut,
  };
}

/** Une ligne de journal : ce qu'on garde d'un appel pour ne pas l'oublier. */
export function resumeVerdict(v: Verdict): string {
  const d = String(v.detail || "").replace(/\s+/g, " ").trim();
  return d.length > 160 ? d.slice(0, 157) + "…" : d;
}

/**
 * ── LE REFUS DE L'INTERFACE EST UNE CATÉGORIE À PART, ET PAS UN REFUS DE PLUS ─
 *
 * Quand le panneau refuse d'émettre (outil non annoncé, champ requis vide, JSON
 * illisible), RIEN n'est parti sur le réseau. Présenter ce refus comme un refus
 * du moteur ferait croire à l'utilisateur que sa machine a parlé, alors qu'elle
 * n'a rien reçu — et le lendemain, il chercherait dans les journaux du nœud une
 * trace qui n'existe pas. C'est la même faute que de taire un refus, dans
 * l'autre sens : attribuer une réponse à qui n'a pas répondu.
 */
export function verdictInterface(motif: string): Verdict {
  return {
    genre: "refus_interface",
    titre: "Refusé par l'interface — rien n'a été envoyé",
    detail: String(motif || "motif absent"),
    couleur: "#c9a227",
  };
}
