/**
 * ── DENSIFIER OU APPROFONDIR — LA LOI QUI MANQUAIT ──────────────────────────
 *
 * Module PUR (aucun import, aucun accès au DOM) : il est testable tel quel, et
 * `densite.test.mjs` le fait. C'est la pratique du dépôt pour les primitives —
 * un calcul qui décide de l'affichage se teste sans navigateur.
 *
 * ── LE DÉFAUT MESURÉ, ET POURQUOI IL RENDAIT LA QUESTION INDÉCIDABLE ────────
 *
 * `/api/viz/space` et `/api/viz/modules` servent jusqu'à **200 ensembles**
 * (`TOP_EFF = 200`, triés par effectif décroissant — `crates/obrain-cli/src/
 * viz_serve.rs`). Le front, lui, COUPAIT À 24 (`FRACTALE.groupesMax`) **avant**
 * de lire la longueur :
 *
 *     it.feuille = estFeuille(modules.map(...), …)   // `modules` déjà coupé
 *     setInterieur({ n: modules.length, total: tousModules.length, … })
 *
 * donc `n` ET `total` valaient 24 dès que le niveau dépassait 24 ensembles. Le
 * panneau possédait la phrase « N sur total » — elle ne pouvait PAS s'afficher,
 * faute d'un total réel. Conséquence : 176 groupes invisibles, sans un mot, et
 * AUCUN geste pour les voir. Un plafond silencieux ne se distingue pas d'une
 * donnée absente — c'est ce que ce fichier refuse.
 *
 * ── LES DEUX GESTES, ET CE QUI LES SÉPARE ───────────────────────────────────
 *
 *   · DENSIFIER — montrer plus de matière **au même niveau** : la page suivante
 *     de ce que l'appel a rendu. L'échelle NE BOUGE PAS (préfixe et rang
 *     inchangés), et le plus souvent AUCUN appel n'est nécessaire : la matière
 *     est déjà dans la réponse, elle n'était simplement pas dessinée.
 *   · APPROFONDIR — descendre d'un CRAN : `prefixe` + « . » + nom, `rang` + 1
 *     (`fractale.ts`), donc un AUTRE appel et une autre échelle. C'est le seul
 *     geste qui existait avant cette loi, et il ne répond pas à « montre-moi ce
 *     qui manque ICI ».
 *
 * ── LA LOI, ET L'ORDRE DE SES RÈGLES ────────────────────────────────────────
 *
 * 1. Si l'intention est EXPLICITE (« dedans » : l'utilisateur a désigné un
 *    groupe), on approfondit — un groupe désigné est une DIRECTION, et densifier
 *    le niveau ne répondrait pas à la question posée.
 * 2. Sinon, tant qu'il reste de la matière NON MONTÉE (`total > montres`), on
 *    DENSIFIE. Jamais approfondir avant : ce serait sauter un cran en laissant
 *    des groupes invisibles — exactement le défaut mesuré (24 montrés, 200
 *    servis).
 * 3. Si tout est monté mais que le SERVEUR a coupé à son plafond
 *    (`plafondAtteint`), la matière existe au-delà : on densifie ENCORE, mais
 *    par un appel (la page suivante du serveur) — on le DIT, parce que ce n'est
 *    pas le même prix.
 * 4. Si le niveau est une FEUILLE et que tout est monté : il n'y a rien de plus,
 *    ni ici ni en dessous. On ne propose rien, et on le dit (`rien`).
 * 5. Sinon (tout est monté, et il reste des sous-groupes dessous) : le seul
 *    « plus » restant est un cran plus bas → APPROFONDIR.
 *
 * L'intention « plus » (le geste qui demande explicitement « davantage ») suit
 * les mêmes règles 2 à 5 : c'est le même calcul, et deux calculs divergeraient.
 */

/** Un geste offert à l'utilisateur, ou « rien » quand il n'y a rien à offrir. */
export type Geste = "densifier" | "approfondir" | "rien";

/** D'où vient la demande : « plus » (densifier), « dedans » (approfondir), ou
 *  ambiguë (`null` : la loi tranche — c'est le cas des gestes génériques). */
export type Intention = "plus" | "dedans" | null;

export interface EtatNiveau {
  /** Ce que l'APPEL a rendu pour ce niveau (le serveur peut avoir coupé). */
  total: number;
  /** Ce qui est DESSINÉ à cet instant. */
  montres: number;
  /** Le niveau n'a plus de sous-groupes exploitables (`fractale.ts`). */
  feuille?: boolean;
  /** Le serveur a lui-même coupé à son plafond : la page suivante est chez lui. */
  plafondAtteint?: boolean;
  /** Ce que l'utilisateur a demandé, quand il l'a dit. */
  intention?: Intention;
  /**
   * ── OÙ VIT LA PAGE SUIVANTE : DEUX DENSIFICATIONS DIFFÉRENTES ────────────
   *
   * Mesuré (2026-09-15) : la descente `Page ↓` sur `runtime-dev` n'atteint
   * JAMAIS un niveau dont la page serveur est pleine (`plafondAtteint` faux du
   * rang 3 au rang 7). La raison est structurelle : à un niveau descendu, les
   * groupes ne viennent pas de l'agrégat paginé mais d'un ÉCHANTILLON DU GRAPHE
   * (l'axe choisi, ou le token suivant des nœuds lus). La règle 3 telle qu'elle
   * était écrite ne pouvait donc pas s'armer là où l'on navigue : « densifier »
   * y restait un redessin, et le « plus » n'existait pas.
   *
   *   · `"serveur"` : l'agrégat paginé — la suite est un `offset` (`/api/viz/space`) ;
   *   · `"graphe"`   : un échantillon du graphe — la suite est un échantillon PLUS
   *     LARGE (plus de centres), au même niveau : ce n'est ni un `offset` ni un cran ;
   *   · `null`/absent : rien de plus (on retombe sur les autres règles).
   */
  source?: "serveur" | "graphe" | null;
  /**
   * Part du monde réellement LUE par l'échantillon du graphe (0..1).
   *
   * C'est la seule grandeur qui dise « il y a plus » quand il n'y a pas de
   * `tronque` : mesuré sur `runtime-dev`, le niveau `packages` (52 979 nœuds
   * réels) est découpé sur un échantillon de 350 nœuds — 0,09 % — et aucun
   * plafond ne le dit. Publier la couverture rend la décision mesurable au lieu
   * de la supposer.
   */
  couverture?: number;
}

export interface Decision {
  geste: Geste;
  /**
   * Combien d'éléments de plus la page suivante montrerait.
   *
   * `undefined` a un sens précis, et il est nécessaire : quand la suite est un
   * ÉCHANTILLON plus large (`source: "graphe"`), on ne peut pas annoncer un
   * nombre — le promettre serait inventer. Les surfaces disent alors « élargir
   * l'échantillon », jamais « +24 de plus ».
   */
  dePlus?: number;
  /** Vrai si le geste change d'ÉCHELLE (préfixe/rang) — la mesure qui sépare
   *  les deux gestes, et ce que le contrôle d'acceptation exige. */
  bougeLeNiveau: boolean;
  /** Vrai si densifier exige un APPEL (la page est au-delà du plafond serveur) :
   *  sans ce champ, on annoncerait « révéler » pour une requête à payer. */
  requiertAppel: boolean;
  /** Pourquoi — la phrase est AFFICHÉE, pas seulement journalisée. */
  raison: string;
}

/** Les pas de pagination d'un niveau : ce qu'on dessine, et ce que l'appel rend. */
export const PAGE = {
  /** Ce qu'un niveau DESSINE à sa première page. */
  pas: 24,
  /**
   * Le plafond d'un agrégat serveur (`/api/viz/space`, `/api/viz/modules`).
   * Au-delà, c'est le SERVEUR qui coupe — et densifier devient un appel.
   */
  plafondAppel: 200,
  /**
   * Couverture en dessous de laquelle un niveau découpé sur un ÉCHANTILLON est
   * réputé densifiable : moins de la moitié du monde est lue, donc élargir
   * l'échantillon montrera des groupes qui existent et qu'on ne voit pas.
   * Au-dessus, l'échantillon est représentatif : densifier n'apporterait rien,
   * et le seul « plus » est un cran plus bas (règle 5).
   */
  couvertureMin: 0.5,
  /** Le nombre de centres d'un graphe de niveau, et son plafond (×2 par
   *  densification) : c'est le seul levier d'un échantillon, `hops` et `limit`
   *  étant déjà au maximum utile (mesuré : `hops=3` coûte 10× pour 0 nœud de plus). */
  centresNiveau: 32,
  centresMax: 256,
} as const;

/** Un état cohérent : on ne dessine jamais plus qu'on n'a reçu. */
const normalise = (e: EtatNiveau) => {
  const total = Math.max(0, Math.floor(e.total) || 0);
  const montres = Math.min(total, Math.max(0, Math.floor(e.montres) || 0));
  return { ...e, total, montres, reste: total - montres };
};

/** La page suivante, bornée par ce qui existe réellement. */
export function pageSuivante(montres: number, total: number, pas = PAGE.pas): number {
  /**
   * ── UN ENTIER PROPRE, OU LA VALEUR PAR DÉFAUT — JAMAIS NaN ────────────────
   *
   * MESURÉ par les tests de coins (2026-09-16) : `Math.max(1, NaN)` vaut **NaN**,
   * donc `pageSuivante(0, 100, NaN)` rendait NaN — et un NaN remonte jusqu'au
   * dessin (on ne dessine jamais « NaN groupes ») sans qu'aucune garde ne le
   * dise. Or ce module a une doctrine explicite : un état impossible est RAMENÉ,
   * jamais cru. Un pas illisible retombe donc sur le pas par défaut, et un
   * `montres` illisible sur « rien de dessiné » (la page suivante est alors une
   * page entière, bornée par le total).
   */
  const entier = (x: number, defaut: number) =>
    (Number.isFinite(x) ? Math.max(0, Math.floor(x)) : defaut);
  const t = entier(total, 0);
  const m = Math.min(t, entier(montres, 0));
  const p = Math.max(1, entier(pas, PAGE.pas));
  return Math.min(t, m + p);
}

/** Y a-t-il de la matière DÉJÀ reçue qui ne soit pas dessinée ? */
export const resteAMontrer = (e: EtatNiveau) => normalise(e).reste;

/** Vrai si densifier montrerait quelque chose de plus (mémoire, serveur, échantillon). */
export function densifiable(e: EtatNiveau): boolean {
  const n = normalise(e);
  return n.reste > 0 || !!e.plafondAtteint || echantillonMaigre(e);
}

/**
 * Vrai si le niveau est découpé sur un échantillon trop maigre pour être cru.
 *
 * On ne demande pas au graphe s'il a « coupé » (il ne le dit pas : mesuré, le
 * niveau `packages` rend 350 nœuds sur 52 979 réels, sans aucun `truncated`) :
 * on compare ce qui est LU à ce que le monde CONTIENT, et c'est la couverture
 * publiée par la surface qui tranche.
 */
export function echantillonMaigre(e: EtatNiveau): boolean {
  return e.source === "graphe" && typeof e.couverture === "number" && e.couverture < PAGE.couvertureMin;
}

/**
 * LA LOI — ce qu'il faut faire quand on demande « davantage ».
 *
 * Le `dePlus` est le nombre d'éléments que la page suivante ajouterait : c'est
 * ce que les surfaces ANNONCENT (« +24 de plus »), parce qu'un geste qui ne dit
 * pas ce qu'il rapporte ne se distingue pas d'un geste décoratif.
 */
export function decisionDensite(etat: EtatNiveau): Decision {
  const n = normalise(etat);
  // 1. Une direction désignée : on va DEDANS (et le niveau bouge).
  if (etat.intention === "dedans") {
    return {
      geste: "approfondir", dePlus: 0, bougeLeNiveau: true, requiertAppel: true,
      raison: "un groupe est désigné : descendre d'un cran répond à la question posée",
    };
  }
  // 2. Il reste de la matière reçue et non montrée : on densifie, sans appel.
  if (n.reste > 0) {
    const page = pageSuivante(n.montres, n.total);
    return {
      geste: "densifier", dePlus: page - n.montres, bougeLeNiveau: false, requiertAppel: false,
      raison: `${n.total} ensemble(s) reçu(s), ${n.montres} dessiné(s) : densifier le MÊME niveau`,
    };
  }
  // 3 bis. Le niveau est découpé sur un ÉCHANTILLON du graphe, et cet échantillon
  //    couvre une part infime du monde : le « plus » existe, mais il n'est ni un
  //    `offset` ni un cran — c'est un échantillon PLUS LARGE, au même niveau.
  //    C'est la densification que la navigation réclame vraiment : mesuré, aucun
  //    niveau atteint par la descente n'a de page serveur pleine (règle 3), donc
  //    sans cette règle « densifier » restait un redessin et la question posée
  //    (« quand densifier l'appel, quand approfondir ») n'avait pas de réponse là
  //    où l'on navigue.
  if (echantillonMaigre(etat)) {
    return {
      geste: "densifier", dePlus: undefined, bougeLeNiveau: false, requiertAppel: true,
      raison: `niveau découpé sur un échantillon (${pourcent(etat.couverture)} du monde lu) : `
        + "densifier = élargir l'échantillon, pas descendre",
    };
  }
  // 3. Tout est montré, mais le SERVEUR a coupé : la page suivante est chez lui.
  //
  // ── LA CONDITION EST `plafondAtteint` SEUL — CORRECTION MESURÉE ──────────
  // Elle exigeait en plus `total >= 200`, et c'était une CONFUSION entre ce que
  // le SERVEUR a coupé et ce que NOUS avons gardé. Or nous filtrons (`tokenValide`,
  // `count > 1`) : mesuré sur `runtime-dev`, le niveau `deepseek_harness` (rang 4)
  // rend 200 lignes avec `tronque: true`, dont un groupe au nom VIDE portant
  // 8 438 nœuds — écarté. Le total filtré retombe sous 200, la règle 3 ne s'armait
  // donc plus, et l'UI annonçait « approfondir » en abandonnant les pages 2..N du
  // serveur : le plafond silencieux, exactement le défaut que ce module corrige,
  // simplement déplacé d'un cran.
  // `plafondAtteint` est CE QUE LE SERVEUR DIT ; le total filtré ne peut pas le
  // contredire. Une page qui ne rapporte rien se traite où on la paie : la surface
  // éteint alors `plafondAtteint` (voir `CosmosWeave.densifierNiveauRef`), et la
  // loi n'a pas à tenir un état qu'elle ne peut pas observer.
  if (etat.plafondAtteint) {
    return {
      geste: "densifier", dePlus: PAGE.pas, bougeLeNiveau: false, requiertAppel: true,
      raison: `le serveur a dit sa page PLEINE (${n.total} gardé(s) après filtres) : la suite est un appel, pas une descente`,
    };
  }
  // 4. Rien ici, rien en dessous : on ne propose rien, et on le dit.
  if (etat.feuille) {
    return {
      geste: "rien", dePlus: 0, bougeLeNiveau: false, requiertAppel: false,
      raison: "niveau FEUILLE entièrement montré : ni au même niveau, ni en dessous",
    };
  }
  // 5. Tout est montré, et il reste des sous-groupes : le seul « plus » est plus bas.
  return {
    geste: "approfondir", dePlus: 0, bougeLeNiveau: true, requiertAppel: true,
    raison: `les ${n.total} ensemble(s) du niveau sont montrés : le « plus » est un cran plus bas`,
  };
}

/**
 * La phrase est écrite UNE fois : la barre d'échelle et le panneau l'affichent
 * tous les deux, et deux formulations divergeraient sur la même décision.
 */
export function phraseDecision(etat: EtatNiveau, d: Decision = decisionDensite(etat)): string {
  const n = normalise(etat);
  if (d.geste === "densifier") {
    // Trois densifications, trois phrases : `dePlus` ABSENT signifie « échantillon
    // plus large », et annoncer un nombre y serait une invention.
    if (d.dePlus === undefined)
      return `＋ densifier : élargir l'échantillon du graphe (${pourcent(etat.couverture)} du monde lu) — le niveau ne change pas`;
    return d.requiertAppel
      ? `＋ densifier : demander les ${d.dePlus} suivants au serveur (plafond de ${PAGE.plafondAppel} atteint)`
      : `＋ densifier : révéler ${d.dePlus} de plus sur ce niveau (${n.montres} sur ${n.total} dessinés)`;
  }
  if (d.geste === "approfondir") {
    return n.reste > 0
      ? `⤵ approfondir : mais ${n.reste} ensemble(s) de CE niveau ne sont pas montrés`
      : "⤵ approfondir : le seul « plus » restant est un cran plus bas";
  }
  return "niveau entier : rien de plus à montrer";
}

/** Un taux lisible (« 0,09 % »), jamais un nombre brut illisible. */
function pourcent(x?: number): string {
  if (typeof x !== "number" || !Number.isFinite(x)) return "part inconnue";
  const p = Math.max(0, Math.min(1, x)) * 100;
  return (p >= 10 ? p.toFixed(0) : p >= 1 ? p.toFixed(1) : p.toFixed(2)) + " %";
}

/**
 * Le libellé court de la barre — même décision, format d'une puce.
 * `total` y est écrit dès qu'il dépasse ce qui est dessiné : c'est la seule
 * façon pour l'utilisateur de savoir qu'un plafond n'a pas mangé la donnée.
 */
export function libelleCourt(etat: EtatNiveau, d: Decision = decisionDensite(etat)): string {
  const n = normalise(etat);
  const compte = n.total > n.montres ? `${n.montres}/${n.total}` : String(n.total);
  if (d.geste === "densifier")
    return d.dePlus === undefined ? `＋ élargir · ${compte}` : `＋ ${d.dePlus} de plus · ${compte}`;
  if (d.geste === "approfondir") return `⤵ approfondir · ${compte}`;
  return `${compte} · rien de plus`;
}
