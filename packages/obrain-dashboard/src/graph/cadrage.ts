/**
 * ── LE CADRAGE : UN CORPS, UNE LOI DE ZOOM, UNE SEULE AUTORITÉ ──────────────
 *
 * Module PUR (zéro dépendance, zéro three.js, zéro React). Il extrait la
 * « référence de cadrage » qui vivait dans `CosmosWeave.tsx` et y ajoute la
 * notion qui manquait : la DEMANDE.
 *
 * LE DÉFAUT QUE CE MODULE FERME, tel qu'il avait été mesuré dans la vue :
 *
 *  1. LA CAMÉRA CHANGEAIT DE CORPS DE RÉFÉRENCE SANS TRANSITIONNE. Passer du
 *     monde (rayon 30) à un groupe (rayon 8,7) faisait sauter d'un coup la
 *     butée de rapprochement (2,4 → 2,2) et le centre d'orbite d'une dizaine
 *     d'unités : sous les doigts, la molette changeait de LOI en plein geste, et
 *     l'œil perdait l'objet qu'il suivait. La butée n'était pas le seul
 *     coupable : le RAYON qui la fonde changeait de 3,45× (30 / 8,7).
 *
 *  2. DEUX ÉCRIVAINS SE DISPUTAIENT `ctrl.target` DANS LA MÊME IMAGE : une
 *     poursuite (la référence suivie) et une animation de vol. Ce qui gagnait
 *     dépendait de l'ORDRE DES INSTRUCTIONS, donc d'un détail d'implémentation
 *     et non d'une décision. Ici, une demande (clic, bouton, vol) n'écrit
 *     JAMAIS la caméra : elle est POSÉE, puis consommée par `avancer`, seul
 *     écrivain. Un seul écrivain à tout instant, par construction.
 *
 * TROIS RÈGLES, ET RIEN D'AUTRE :
 *   - aucune distance absolue : butée et distance de cadrage sont des MULTIPLES
 *     du rayon du corps visé (les planchers sont les seules constantes, et ils
 *     sont documentés et justifiés) ;
 *   - le rayon s'interpole en LOGARITHME, la position et `frac` en linéaire, à
 *     taux indépendant du framerate ;
 *   - `demander` ne touche pas `actuel` ; `avancer` est le seul à le faire.
 */

export type Genre = "cosmos" | "monde" | "groupe" | "noeud";

export interface Cadrage {
  /** Centre du corps visé (coordonnées monde). */
  pos: { x: number; y: number; z: number };
  /** Rayon du corps visé (unités monde). */
  rayon: number;
  /** Part du rayon qui sert de butée de rapprochement (0.08 monde, 0.25 groupe). */
  frac: number;
  genre: Genre;
  nom: string;
  /** Faux tant que rien n'a été posé : la première pose est directe, sans transition. */
  init: boolean;
}

/**
 * ── LES CONSTANTES, TOUTES JUSTIFIÉES PAR UNE MESURE OU PAR UNE STRUCTURE ───
 *
 * Elles sont PRIVÉES à dessein : l'interface publique ne donne que des lois.
 * Un appelant qui aurait besoin de régler l'une d'elles doit passer par le
 * paramètre prévu (`facteur` de `distanceCadrage`, `seuil` de `stabilise`).
 */

/**
 * Taux de transition, en 1/seconde. C'est la seule vitesse du module, et elle
 * est indifférente au framerate :
 *
 *   k = 1 − exp(−dt × TAUX)
 *
 * Valeur 2,2 : celle de la version embarquée, mesurée. Elle donne, à 60 i/s,
 * un pas de k = 0,036 — soit 4,36 % de variation de rayon par image sur une
 * transition monde → groupe (voir le test : la borne de continuité de 5 % par
 * pas est tenue, il s'en faut de peu, et c'est VOULU : plus lent, on attend ;
 * plus rapide, la transition se voit comme un saut).
 */
const TAUX = 2.2;

/**
 * Part du rayon qui sert de butée de rapprochement, par genre. 0,08 pour un
 * monde (« on s'arrête à son cœur »), 0,25 pour un groupe et un nœud (« on doit
 * pouvoir ENTRER dedans »).
 */
const FRACS: Record<Genre, number> = {
  cosmos: 0.08,
  monde: 0.08,
  groupe: 0.25,
  noeud: 0.25,
};

/**
 * PLANCHERS de butée, en unités monde — la SEULE constante de distance du
 * module, et elle est un garde-fou, pas une loi de zoom.
 *
 *   1,2 pour un monde et pour le cosmos : valeur mesurée dans la version
 *   embarquée (`centreRef ? 0.4 : 1.2`). Sans elle, un monde de rayon 1 serait
 *   visitable à 0,08 unité, et `lookAt` dégénère quand la caméra tombe sur son
 *   propre centre d'orbite.
 *
 *   0,4 pour un groupe : valeur mesurée elle aussi. C'est ce plancher qui a
 *   corrigé le blocage documenté — « la molette se bloque à 2,4 unités pour un
 *   groupe de rayon 8,7 et l'on reste dehors ».
 *
 *   0,05 pour un nœud : ASSUMÉ SANS MESURE. Un nœud se visite à l'INTÉRIEUR, et
 *   un plancher de 0,4 (celui du groupe) serait plus grand que le nœud
 *   lui-même : il interdirait d'y entrer, exactement le défaut mesuré pour les
 *   mondes (« la borne de 90 bloquait la caméra à 2,2 rayons de monde »).
 *   0,05 reste non nul, donc jamais de division par zéro ni de `lookAt`
 *   dégénéré. À confirmer par mesure le jour où la vue visitera des nœuds.
 */
const PLANCHERS: Record<Genre, number> = {
  cosmos: 1.2,
  monde: 1.2,
  groupe: 0.4,
  noeud: 0.05,
};

/**
 * Multiples de cadrage, en RAYONS du corps visé : la distance d'arrivée d'un
 * vol, jamais une distance absolue. Les trois premiers sont mesurés :
 *
 *   cosmos 2,0 — `HOME_POS` vaut (0, 520, 1180), soit 1 290 unités pour un
 *     système de ~1 300 unités de diamètre (rayon ~650) : le recul vaut deux
 *     rayons de cosmos, et c'est ce recul qui a remplacé les 1 600 unités où
 *     une sphère de 20 unités ne faisait plus que ~10 px.
 *
 *   monde 8,8 — la visée de `gotoWorld` est `P + (0, size×3,2, size×8,2)`, dont
 *     la norme vaut `size × √(3,2² + 8,2²) = size × 8,80`. C'est LE chiffre qui
 *     a corrigé le défaut documenté : l'ancienne cible (3,4 rayons) collait la
 *     caméra à la surface, la descente n'avait plus aucun trajet à parcourir et
 *     sautait d'un coup à 100 % au lieu d'être continue.
 *
 *   groupe 4,2 — `CADRAGE_POUPEE`. À 4,2 rayons de groupe, la nuée occupe ~1/4
 *     du champ, ses nœuds ~1/4, et la paroi du monde l'entoure : le groupe est
 *     vu DANS son monde sans refermer l'étage.
 *
 *   noeud 4,2 — la même loi, un cran plus bas : un nœud est une poupée
 *     d'échelle inférieure, on le visite avec la même part de champ. Aucun
 *     chiffre mesuré pour ce genre non plus (même réserve que pour son
 *     plancher).
 */
const FACTEURS: Record<Genre, number> = {
  cosmos: 2.0,
  monde: 8.8,
  groupe: 4.2,
  noeud: 4.2,
};

/**
 * Seuil de stabilisation par défaut : 0,02 en ÉCART D'ÉCHELLE, c'est-à-dire un
 * rapport de 1,0202 entre le rayon courant et le rayon visé. Chiffre sans
 * dimension, donc valable à toute échelle — c'est tout l'intérêt. Il est
 * mesuré : une transition monde (30) → groupe (8,7) part d'un écart de
 * ln(30/8,7) = 1,238 et le franchit en 1,88 s à 60 i/s, donc en moins de 2 s.
 * À l'œil, 2 % d'écart de rayon ne se voit pas ; 1 % demanderait 2,19 s.
 */
const SEUIL_STABILISE = 0.02;

/**
 * Rayon minimal utilisé dans les LOGARITHMES et dans les divisions. `ln(0)` vaut
 * −∞ : sans cette borne, un rayon nul rendrait l'écart infini et la transition
 * éternelle. 1e-3 est la valeur de la version embarquée.
 */
const RAYON_MIN = 1e-3;

/* ── OUTILS PRIVÉS : aucune entrée hostile ne doit produire de NaN ─────────── */

/** Un nombre utilisable comme rayon : fini, strictement positif, borné en bas. */
function rayonSur(r: number): number {
  // Un rayon non fini ou nul n'a pas d'échelle : on retombe sur la borne basse.
  // Le résultat est TOUJOURS dans [RAYON_MIN, +∞[, donc les log et les
  // divisions de ce module ne peuvent plus produire d'infini ni de NaN.
  return Number.isFinite(r) && r > RAYON_MIN ? r : RAYON_MIN;
}

/** Part de butée d'un genre. Un genre inconnu (appel JS) retombe sur le monde. */
function partDe(genre: Genre): number {
  return (FRACS as Record<string, number>)[genre] ?? FRACS.monde;
}

/** Plancher d'un genre. Un genre inconnu (appel JS) retombe sur le monde. */
function plancherDe(genre: Genre): number {
  return (PLANCHERS as Record<string, number>)[genre] ?? PLANCHERS.monde;
}

/** Multiple de cadrage d'un genre. Un genre inconnu retombe sur le monde. */
function facteurDe(genre: Genre): number {
  return (FACTEURS as Record<string, number>)[genre] ?? FACTEURS.monde;
}

/**
 * `frac` utilisable d'un cadrage : fini, dans ]0, 1], et sinon la part
 * canonique de son genre. Même raison que `rayonSur` : `frac` multiplie le
 * rayon, donc un `frac` NaN rendrait la butée NaN et la caméra libre de passer
 * sous le corps visé.
 */
function fracSur(c: Cadrage): number {
  return Number.isFinite(c.frac) && c.frac > 0 ? Math.min(1, c.frac) : partDe(c.genre);
}

/** Composante de position utilisable : un non-fini est ramené à l'origine. */
function composanteSur(v: number): number {
  return Number.isFinite(v) ? v : 0;
}

/** Copie d'un cadrage — jamais de mutation de l'entrée, jamais de partage. */
function copie(c: Cadrage): Cadrage {
  return {
    pos: {
      x: composanteSur(c.pos?.x),
      y: composanteSur(c.pos?.y),
      z: composanteSur(c.pos?.z),
    },
    rayon: rayonSur(c.rayon),
    frac: fracSur(c),
    genre: c.genre,
    nom: typeof c.nom === "string" ? c.nom : "",
    init: !!c.init,
  };
}

/* ── LES LOIS DU CORPS VISÉ ────────────────────────────────────────────────── */

/** Distance de butée : jamais un nombre absolu, toujours dérivé du corps visé. */
export function butee(c: Cadrage): number {
  // POURQUOI un max et non une somme : le plancher n'ajoute rien à la loi, il
  // empêche seulement la butée de tomber à zéro sur un corps minuscule. Au-
  // dessus du plancher, la butée est EXACTEMENT proportionnelle au rayon — donc
  // identique à toute échelle, et sans constante absolue.
  return Math.max(plancherDe(c.genre), rayonSur(c.rayon) * fracSur(c));
}

/** Distance de cadrage souhaitée pour un corps (arrivée d'un vol). */
export function distanceCadrage(c: Cadrage, facteur?: number): number {
  // Un `facteur` non fourni, non fini ou négatif retombe sur la loi du genre :
  // la distance reste un MULTIPLE du rayon, jamais une valeur en unités.
  const f = Number.isFinite(facteur) && (facteur as number) > 0 ? (facteur as number) : facteurDe(c.genre);
  return rayonSur(c.rayon) * f;
}

/**
 * Le VRAI cadrage d'un corps à partir de son genre et de son rayon.
 *
 * `init: true` : ce cadrage désigne un corps RÉEL, donc il est posé. Pour
 * représenter « rien n'a encore été posé », l'appelant part de ce cadrage et
 * met `init` à faux (`{ ...cadragePour(...), init: false }`) : la première pose
 * sera alors directe, sans transition, puisqu'il n'y a rien à quitter.
 */
export function cadragePour(
  genre: Genre,
  nom: string,
  pos: { x: number; y: number; z: number },
  rayon: number,
): Cadrage {
  return {
    pos: {
      x: composanteSur(pos?.x),
      y: composanteSur(pos?.y),
      z: composanteSur(pos?.z),
    },
    rayon: rayonSur(rayon),
    frac: partDe(genre),
    genre,
    nom: typeof nom === "string" ? nom : "",
    init: true,
  };
}

/* ── LA TRANSITION ─────────────────────────────────────────────────────────── */

/**
 * UN PAS de transition, à taux indépendant du framerate.
 * Le RAYON est interpolé en LOGARITHME (chaque doublement d'échelle prend la
 * même durée), la position en linéaire, `frac` avec la même constante que la
 * position. Rend un NOUVEAU cadrage (pas de mutation de l'entrée).
 */
export function etape(actuel: Cadrage, voulu: Cadrage, dt: number): Cadrage {
  // 1. AUCUN TEMPS ÉCOULÉ : rien ne bouge. Testé sur `dt` AVANT tout calcul,
  //    parce qu'un `dt` NaN propagerait NaN dans le logarithme et dans la
  //    position, c'est-à-dire dans la caméra.
  if (!Number.isFinite(dt) || dt <= 0) return copie(actuel);

  // 2. RIEN N'EST POSÉ EN FACE : il n'y a pas de cible, donc rien à rejoindre.
  //    `init` faux est le contraire d'un corps : transitionner vers lui serait
  //    transitionner vers le néant (position 0, rayon 1).
  if (!voulu.init) return copie(actuel);

  // 3. PREMIÈRE POSE : DIRECTE. Il n'y a rien à quitter — l'état courant n'est
  //    pas un corps mais un germe. Interpoler depuis ce germe produirait
  //    exactement le saut qu'on veut éviter, mais au tout premier frame.
  if (!actuel.init) return copie(voulu);

  // 4. INTERPOLATION. `k` est la fraction du chemin restant parcourue pendant
  //    `dt` : elle est INDÉPENDANTE DU FRAMERATE, parce que la composition de
  //    deux pas vaut le pas de la durée totale —
  //        (1 − k1)(1 − k2) = exp(−dt1·TAUX)·exp(−dt2·TAUX) = exp(−(dt1+dt2)·TAUX)
  //    ce qui n'est vrai ni d'un lerp à `k` constant, ni d'un pas fixe par
  //    image. Le test le vérifie : une suite de pas de durées différentes mène
  //    au même état qu'un pas unique de durée totale (à 1e-9 près, c'est-à-dire
  //    à l'arrondi flottant près).
  const k = 1 - Math.exp(-dt * TAUX);

  // Le logarithme est ce qui rend la transition CONTINUE À L'ŒIL : de 30 à 8,7,
  // la caméra traverse les deux échelles au même rythme qu'un doublement de
  // distance, au lieu de parcourir 21,3 unités à vitesse constante en donnant
  // l'impression de ne rien faire au début et de tout faire à la fin.
  const lr = Math.log(rayonSur(actuel.rayon));
  const lv = Math.log(rayonSur(voulu.rayon));
  const rayon = Math.exp(lr + (lv - lr) * k);

  const px = composanteSur(actuel.pos?.x);
  const py = composanteSur(actuel.pos?.y);
  const pz = composanteSur(actuel.pos?.z);
  const vx = composanteSur(voulu.pos?.x);
  const vy = composanteSur(voulu.pos?.y);
  const vz = composanteSur(voulu.pos?.z);

  const fa = fracSur(actuel);
  const fv = fracSur(voulu);

  return {
    pos: {
      x: px + (vx - px) * k,
      y: py + (vy - py) * k,
      z: pz + (vz - pz) * k,
    },
    rayon,
    // `frac` transitionne AVEC la position : sans cela, la butée sauterait de
    // 0,08 à 0,25 rayon en une image — la loi de zoom changerait sous les
    // doigts, ce qui était précisément le défaut d'origine.
    frac: fa + (fv - fa) * k,
    // Le genre et le nom ne s'interpolent PAS : ce sont des étiquettes, pas des
    // grandeurs. Ils suivent la cible dès le premier pas, pendant que ce qui
    // fonde la loi de zoom (rayon, frac) transitionne. C'est ce qu'il faut : la
    // butée ne saute pas, mais l'étiquette dit déjà où l'on va.
    genre: voulu.genre,
    nom: voulu.nom,
    init: true,
  };
}

/** Écart restant en échelle (0 = la transition est terminée). */
export function ecartTransition(actuel: Cadrage, voulu: Cadrage): number {
  // Un corps non posé n'a pas d'échelle : l'écart est infini, donc la
  // transition n'est jamais finie. C'est la traduction honnête de « il n'y a
  // pas de cible », et cela évite qu'un appelant croie la transition terminée
  // alors qu'il n'a jamais rien visé.
  if (!voulu.init) return Infinity;
  // L'écart est mesuré sur le RAYON, en logarithme : c'est la seule mesure qui
  // vaut à toute échelle (1,02 entre 8,7 et 8,87 vaut 1,02 entre 30 et 30,6).
  //
  // POURQUOI le rayon suffit à dire « la transition est finie » : la position
  // et `frac` se referment avec le MÊME `k`, donc leur écart relatif décroît au
  // même rythme que celui du rayon. Les trois se terminent ensemble ; mesurer
  // l'échelle, c'est mesurer la transition entière.
  return Math.abs(Math.log(rayonSur(voulu.rayon)) - Math.log(rayonSur(actuel.rayon)));
}

/** Vrai quand l'écart est sous un seuil : la transition d'échelle est finie. */
export function stabilise(actuel: Cadrage, voulu: Cadrage, seuil?: number): boolean {
  // Un seuil non fourni, négatif ou non fini retombe sur la loi du module :
  // un seuil de 0 ne serait jamais atteint par une exponentielle (elle tend vers
  // zéro sans l'atteindre), et la transition ne serait jamais dite finie.
  const s = Number.isFinite(seuil) && (seuil as number) >= 0 ? (seuil as number) : SEUIL_STABILISE;
  return ecartTransition(actuel, voulu) <= s;
}

/* ── L'AUTORITÉ DE TRANSITION ──────────────────────────────────────────────── */

/**
 * Un seul état vivant, une seule fonction qui l'avance. Une DEMANDE (posée par
 * un clic, un changement d'étage, un vol) n'écrit PAS la caméra : elle est
 * consommée ici, et c'est ce qui garantit qu'un seul écrivain existe à tout
 * instant. `vol` (animation) prime sur la poursuite tant qu'il tourne.
 */
export interface EtatCadrage {
  actuel: Cadrage;
  voulu: Cadrage;          // ce que la sélection demande
  /** Demande explicite d'un geste (clic, bouton) : elle prime sur `voulu`. */
  demande: Cadrage | null;
}

/**
 * Pose une demande. Ne touche NI `actuel`, NI `voulu` : une demande est une
 * INTENTION, pas une écriture. L'écriture aura lieu dans `avancer`, une fois par
 * image, et c'est pour cela que deux gestes dans la même image ne peuvent plus
 * se disputer la caméra — le dernier posé gagne, et il gagne AVANT le rendu.
 *
 * Une demande ABOUTIE devient la référence : c'est `avancer` qui la recopie
 * dans `voulu` en la consommant, et c'est ce qui évite le retour en arrière
 * (voir `avancer`).
 */
export function demander(e: EtatCadrage, c: Cadrage): EtatCadrage {
  // On copie la demande : un appelant qui modifierait ensuite son propre objet
  // (une `Vector3` réutilisée, par exemple) ne pourra pas changer la cible en
  // cours de transition. `actuel` et `voulu` sont rendus tels quels, référence
  // comprise : `demander` ne prend jamais la plume de `avancer`.
  return { actuel: e.actuel, voulu: e.voulu, demande: copie(c) };
}

/**
 * UN PAS D'AUTORITÉ : le seul endroit du module où `actuel` change, et le seul
 * qui dise QUI a écrit.
 *
 *   "vol"       — une demande est en cours et prime sur la sélection ;
 *   "poursuite" — la sélection (`voulu`) est rejointe ;
 *   "immobile"  — rien n'a été écrit : pas de temps écoulé, pas de cible posée,
 *                 ou écart déjà nul (auquel cas une demande arrivée à son but
 *                 est consommée ici, et nulle part ailleurs).
 */
export function avancer(e: EtatCadrage, dt: number): { etat: EtatCadrage; ecrivain: "vol" | "poursuite" | "immobile" } {
  // 1. AUCUN TEMPS ÉCOULÉ (dt nul, négatif ou non fini) : on rend l'ÉTAT
  //    LUI-MÊME, pas une copie. « Ne change rien » doit être vrai en valeur ET
  //    en identité : un appelant qui compare les références voit qu'il ne s'est
  //    rien passé, et rien n'est consommé — une demande posée pendant une image
  //    de durée nulle reste en attente, elle n'est pas perdue.
  if (!Number.isFinite(dt) || dt <= 0) return { etat: e, ecrivain: "immobile" };

  // 2. LA DEMANDE PRIME SUR LA SÉLECTION. C'est tout l'objet du champ : un clic
  //    sur un groupe ne doit pas être annulé par la poursuite du monde que la
  //    sélection réclame. Un vol est une demande comme une autre ; il prime donc
  //    tant qu'il tourne, et cesse de primer quand il est arrivé.
  const cible = e.demande ?? e.voulu;
  if (!cible.init) return { etat: e, ecrivain: "immobile" };

  // 3. ÉCART DÉJÀ NUL : la transition est finie. C'est ICI que la demande est
  //    consommée — à son arrivée, jamais avant — et qu'elle DEVIENT la
  //    référence (`voulu`). Sans cette adoption, la caméra repartirait vers la
  //    sélection d'avant le geste sitôt le geste fini : un clic sur un groupe
  //    serait suivi d'un vol de retour vers le monde, c'est-à-dire exactement
  //    le « j'avance et je repars en arrière » documenté dans la vue.
  if (stabilise(e.actuel, cible)) {
    return {
      etat: e.demande ? { actuel: e.actuel, voulu: e.demande, demande: null } : e,
      ecrivain: "immobile",
    };
  }

  // 4. UN PAS, UN ÉCRIVAIN. Tant que la demande tourne, `voulu` n'est pas
  //    touché : la sélection reste ce qu'elle était, la demande est une couche
  //    par-dessus, et elle prime.
  const actuel = etape(e.actuel, cible, dt);
  if (e.demande) {
    // CONSOMMATION À L'ARRIVÉE, et à ce moment-là seulement : la transition est
    // finie, la demande a tenu sa promesse et devient la référence.
    const arrive = stabilise(actuel, cible);
    return {
      etat: { actuel, voulu: arrive ? e.demande : e.voulu, demande: arrive ? null : e.demande },
      ecrivain: "vol",
    };
  }
  return { etat: { actuel, voulu: e.voulu, demande: null }, ecrivain: "poursuite" };
}
