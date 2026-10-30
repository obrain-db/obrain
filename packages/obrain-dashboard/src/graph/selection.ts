/**
 * SÉLECTION PAR POINTAGE — arithmétique PURE, entièrement en PIXELS.
 *
 * Aucun import : ni three.js, ni React. Que des nombres. C'est ce qui rend la
 * décision DÉCIDABLE hors navigateur — et donc testable (voir
 * `selection.test.mjs`), ce que le `Raycaster` de `CosmosWeave.tsx` n'a jamais
 * permis : ses quatre fautes n'ont été trouvées qu'à l'œil, une par une.
 *
 * ── CE QUE CE MODULE REMPLACE, ET POURQUOI ──────────────────────────────────
 *
 * 1. `hits[0]`. L'ancien code ne regardait que le premier objet touché par le
 *    rayon et rendait `undefined` s'il ne portait pas d'identité résoluble : un
 *    clic sur un objet parfaitement désignable ne faisait RIEN. Ici, TOUS les
 *    candidats sont évalués, puis classés — il n'y a plus de « premier ».
 *
 * 2. Des seuils en UNITÉS-MONDE. `ray.params.Points.threshold = 7` et
 *    `Line.threshold = 4` (CosmosWeave, lignes 1293-1296) sont des longueurs
 *    dans la scène, pas des tolérances de main. Avec la loi d'échelle du dépôt
 *    lui-même (`echelle = 2·tan(fov/2)·dist/hauteur`, ligne 1578) et les
 *    réglages réels de la vue (fov 60, hauteur 600 px — lignes 72-80 de
 *    `grammar.ts`, ligne 779 de `CosmosWeave.tsx`) :
 *        · à 100 unités  : 7 unités = 36,4 px d'écran ;
 *        · à 1200 unités : 7 unités =  3,0 px ;
 *        · à 4000 unités : 7 unités =  0,9 px.
 *    Le MÊME nombre vaut 40× plus de pixels selon l'endroit où l'on se trouve
 *    dans la scène — il n'exprime donc aucune tolérance de visée. Ici, la
 *    tolérance est en pixels et ne dépend plus de l'endroit : c'est elle, et
 *    elle seule, qui décide.
 *
 * 3. Rien sur « ce que l'utilisateur regarde ». Le rayon ne connaissait ni
 *    l'empreinte à l'écran, ni la visibilité, ni l'étage. Ici : l'empreinte se
 *    mesure (projection de deux points), la visibilité pondère, l'étage prime.
 *
 * 4. La FACE ARRIÈRE d'une sphère gagnait. Une sphère touchée par un rayon
 *    renvoie DEUX intersections ; depuis l'intérieur d'un monde, la seule
 *    surface offerte au pointeur est la paroi arrière — d'où la cage cliquée à
 *    la place du groupe situé dedans. Règle retenue (voir `FACE ARRIÈRE`
 *    ci-dessous) : une boule n'offre une face avant que si la caméra est DEHORS.
 *
 * ── POURQUOI DES PIXELS : CE N'EST PAS UNE PRÉFÉRENCE, C'EST UN PRÉCÉDENT ───
 *
 * Le dépôt a déjà tranché cette question ailleurs, dans le même fichier :
 * « LA BONNE UNITÉ : LA TAILLE À L'ÉCRAN, PAS LES RAYONS » (ligne 3833) — les
 * seuils en rayons de monde « ignoraient donc la taille de l'écran », et la
 * correction fut de mesurer une part de l'écran. Le cadrage des niveaux projette
 * déjà deux points pour ça (`rayonEcran`, ligne 3608). Ce module applique la
 * même loi à la sélection, en pixels plutôt qu'en fraction de hauteur.
 *
 * ── L'EMPREINTE, ET SON SEUL ÉCART CONNU ────────────────────────────────────
 *
 * `empreinte` = distance à l'écran entre le centre projeté et un point décalé du
 * rayon, PERPENDICULAIREMENT à l'axe optique (ligne 0 de la matrice de vue).
 * L'ancien `rayonEcran` décale le long de l'axe MONDE +Y (ligne 3610) : depuis
 * la vue Cosmos, la caméra est à (0, 520, 1180) et regarde l'origine, donc
 * l'axe optique a une composante Y de −0,403 — le décalage « +Y » mesure alors
 * 0,915·r en latéral et change la profondeur au passage. Vu de dessus (caméra
 * au pôle), il mesure ZÉRO pour une boule de rayon r : elle est vue comme un
 * point. Un décalage perpendiculaire à l'axe optique ne peut pas dégénérer.
 *
 * Écart assumé de cette mesure (documenté pour ne pas le redécouvrir) : le
 * rayon d'écran vaut tan(atan(r/d)) par décalage latéral, contre
 * tan(asin(r/d)) pour la silhouette exacte d'une sphère — soit un rapport
 * √(1 − (r/d)²). La mesure SOUS-estime donc de 1,1 % à r/d = 0,15, de 4,6 % à
 * 0,3 et de 13,4 % à 0,5. L'erreur est du côté prudent (on n'invente jamais de
 * surface), et le facteur 2,2 du rayon de sélection la couvre plusieurs fois.
 * Les tests vérifient ce rapport, plutôt que de le supposer.
 *
 * ── FACE ARRIÈRE : LA RÈGLE RETENUE, ET POURQUOI CELLE-LÀ ───────────────────
 *
 *   · ancre DERRIÈRE la caméra (w ≤ 0) → aucune position écran, donc aucune
 *     mesure en pixels possible → NON DÉSIGNABLE. On ne devine pas une position.
 *   · caméra DANS la boule (distance caméra↔ancre ≤ rayon) → la seule surface
 *     que le pointeur peut atteindre est la paroi OPPOSÉE, c'est-à-dire la face
 *     arrière : la boule n'a plus de face avant → pénalité ×0,08 (elle RESTE un
 *     repli, elle n'est jamais ignorée : sinon un clic dans un monde vide ne
 *     ferait rien, le défaut même qu'on corrige).
 *   · caméra DEHORS → l'empreinte projetée est bien l'image de l'hémisphère
 *     avant : aucune pénalité.
 * Le critère est géométrique (une distance et un rayon), pas un signe de produit
 * scalaire : il ne dépend donc ni de l'orientation de la caméra ni d'un axe
 * privilégié, et il se teste sans navigateur.
 *
 * ── LA FORMULE DE SCORE, EN ENTIER ──────────────────────────────────────────
 *
 *   score = base · proxi · prec · vis · etage · face            (base = 100)
 *
 *   portee = empreinte + rayonPx          tolérance totale en px (le disque de
 *                                         l'objet PLUS la marge de visée)
 *   proxi  = portee / (portee + distAncre)   ∈ (0, 1]
 *       1 quand le pointeur est sur l'ancre, 0,5 à la limite de portée. C'est
 *       une distance EN PIXELS, jamais en unités monde ; le dénominateur est
 *       strictement positif (rayonPx ≥ plancherPx), donc jamais de division
 *       par zéro.
 *   prec   = plafondPx / (plafondPx + empreinte)   ∈ (0, 1]
 *       À mise égale, la cible la plus PETITE gagne : elle est plus précise à
 *       viser. Facteur borné (≈ ×2 au maximum sur la plage utile), c'est un
 *       départage, pas un renversement.
 *   vis    = 0,25 + 0,75 · opacite                 ∈ [0,25, 1]
 *       L'opacité est celle RÉELLEMENT appliquée à l'écran. Ce n'est pas un
 *       détail : dans ce dépôt, la nuée d'un monde qu'on ouvre tombe à
 *       0,96 · (1 − 0,85·ouv) = 0,144 — SOUS le seuil de 0,15 (ligne 3868) —
 *       et la cage d'un monde est un filaire à 0,10 (ligne 1085), tandis que
 *       les groupes intérieurs reçoivent `ouv · base`, donc 0 quand ils ne sont
 *       pas montrés (ligne 3885). Sans cette pondération, on clique des objets
 *       qui n'ont plus de pixels.
 *   etage  = 8 si (etage ∈ etages ET focal)
 *            1 si (etage ∈ etages)
 *            0,2 sinon                                     ← repli, jamais ignoré
 *   face   = 1 si rayon = 0 ou caméra DEHORS ; 0,08 si caméra DANS la boule.
 *
 * Ordre de grandeur : score ∈ (0, 800]. Un candidat parfaitement visé, opaque,
 * focal, légitime vaut ≈ 100 · 1 · 0,9 · 1 · 8 = 720. La constante 100 n'est
 * qu'une échelle de lisibilité (des nombres à trois chiffres se comparent à
 * l'œil dans `raisons`).
 *
 * ── L'INVARIANT GARANTI (et vérifié par les tests) ──────────────────────────
 *
 * « L'étage prime » n'est pas une tendance, c'est une INÉGALITÉ. Pour deux
 * candidats désignables du même étage :
 *     proxi  : au pire 2      (pointeur au bord de la portée contre pointeur sur l'ancre)
 *     prec   : au pire 2      (empreinte au plafond contre empreinte nulle)
 *     vis    : égal           (même visibilité supposée)
 * donc le produit des facteurs de forme et de distance vaut au pire 4, tandis
 * que le passage de « non focal » (×1) à « focal » (×8) vaut 8. Un candidat
 * focal bat donc TOUJOURS un candidat non focal un peu mieux placé — la marge
 * est de 2× dans le pire cas construit. Seule une visibilité franchement
 * meilleure (×4 possible) peut renverser ce rapport, ce qui est voulu : une
 * cible qu'on ne voit pas ne doit pas voler la place d'une cible qu'on voit.
 *
 * ── ROBUSTESSE ──────────────────────────────────────────────────────────────
 *
 *   · tableau vide ou caméra inutilisable → `[]` (et `null` pour `choisir`) ;
 *     une caméra inutilisable ne rend AUCUN classement plutôt qu'un faux ;
 *   · coordonnées non finies (NaN, ±∞) → le candidat est IGNORÉ, il ne figure
 *     pas dans le classement : le compter avec un score 0 laisserait croire
 *     qu'il a été évalué ;
 *   · aucune division par zéro (dénominateurs minorés) ; `Math.hypot` et
 *     `Number.isFinite` gardent les sorties finies ;
 *   · réglages absurdes (0, négatifs, NaN) → ramenés aux valeurs par défaut.
 */

// ---------------------------------------------------------------------------
// INTERFACE — les types attendus par l'appelant, tels quels.
// ---------------------------------------------------------------------------

export type Genre = "monde" | "ensemble" | "noeud" | "arete";

export interface Candidat {
  genre: Genre;
  qn: string; // monde ou graphe propriétaire
  nom: string; // libellé (nom d'ensemble, qualified_name de nœud…)
  nom2?: string; // seconde extrémité pour une arête
  /** Point d'ancrage en coordonnées MONDE (centre de la boule, position du nœud…). */
  ancre: { x: number; y: number; z: number };
  /** Rayon apparent en unités monde (0 pour un point). */
  rayon: number;
  /** Opacité RÉELLEMENT appliquée à l'écran, 0..1. */
  opacite: number;
  /** Étages où ce candidat est légitime (0 cosmos, 1 monde, 2 graphe). */
  etages: number[];
  /** Vrai si le candidat appartient à l'étage actuellement focalisé. */
  focal: boolean;
}

export interface Camera {
  position: { x: number; y: number; z: number };
  /** Quaternion ou base : fournis la matrice de vue sous forme de 16 nombres. */
  viewProjection: number[]; // matrice 4x4 colonne-major (comme three.js)
  largeur: number; // px
  hauteur: number; // px
}

export interface Classement {
  candidat: Candidat;
  /**
   * Distance au pointeur EN PIXELS (jamais en unités monde).
   * C'est la distance à l'EMPREINTE de l'objet (son disque à l'écran), pas à
   * son ancre : elle vaut 0 dès que le pointeur est sur l'objet, ce qui est la
   * définition utile — pointer le bord d'un monde, c'est le pointer, lui.
   * `Infinity` dans un seul cas : ancre derrière la caméra, donc distance non
   * mesurable (cette valeur coïncide toujours avec score = 0).
   */
  px: number;
  /** Rayon de sélection effectif en pixels (fonction de l'empreinte du candidat). */
  rayonPx: number;
  /**
   * Score : plus grand = meilleur. Formule complète et bornes en tête de
   * fichier. `score > 0` ⟺ le candidat est DÉSIGNABLE ; les facteurs étant tous
   * strictement positifs, un score nul ne peut venir que d'un veto explicite
   * (opacité sous le seuil, hors de portée, ancre derrière la caméra).
   */
  score: number;
  /** Pourquoi ce score (traçabilité) : un libellé par facteur appliqué. */
  raisons: string[];
}

/**
 * Réglages — « plancher configurable (6 px par défaut) », seuil d'opacité, etc.
 * Passés en 5e argument OPTIONNEL de `classer` / `choisir` (l'appel à quatre
 * arguments reste exactement celui de l'interface) : la fonction reste pure, et
 * deux appels avec les mêmes entrées rendent le même résultat.
 */
export interface Reglages {
  /** Empreinte plancher d'un point (nœud, arête) : un nœud n'a pas de rayon monde. */
  plancherPx: number;
  /** Marge de visée : multiple de l'empreinte. */
  facteur: number;
  /** Plafond du rayon de sélection (px) — et échelle du facteur `prec`. */
  plafondPx: number;
  /** Sous cette opacité, le candidat n'est pas désignable du tout. */
  seuilOpacite: number;
  /** Poids de l'opacité : `vis = poidsMin + (1 − poidsMin)·opacite`. */
  poidsMinOpacite: number;
  /** Bonus d'un candidat légitime à l'étage courant ET focal. */
  bonusFocal: number;
  /** Pénalité d'un candidat hors de l'étage courant (repli, jamais ignoré). */
  penaliteHorsEtage: number;
  /** Pénalité d'une boule dont la caméra est à l'intérieur (face arrière seule). */
  penaliteFaceArriere: number;
  /** Constante d'échelle du score, pour des nombres lisibles. */
  base: number;
}

export const REGLAGES_DEFAUT: Readonly<Reglages> = Object.freeze({
  plancherPx: 6,
  facteur: 2.2,
  plafondPx: 160,
  seuilOpacite: 0.15,
  poidsMinOpacite: 0.25,
  bonusFocal: 8,
  penaliteHorsEtage: 0.2,
  penaliteFaceArriere: 0.08,
  base: 100,
});

// ---------------------------------------------------------------------------
// OUTILLAGE INTERNE — aucune dépendance, aucune allocation inutile.
// ---------------------------------------------------------------------------

type P3 = { x: number; y: number; z: number };

/** Marge sur w : en deçà, le point est sur la caméra (division explosive). */
const W_MIN = 1e-6;

function estFini(n: number | undefined | null): n is number {
  return typeof n === "number" && Number.isFinite(n);
}

function pointFini(p: P3 | undefined | null): p is P3 {
  return !!p && estFini(p.x) && estFini(p.y) && estFini(p.z);
}

function borne(n: number, lo: number, hi: number): number {
  return n < lo ? lo : n > hi ? hi : n;
}

/** Espace monde entre deux points (unités monde — sert à la seule règle de face). */
function distMonde(a: P3, b: P3): number {
  return Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z);
}

/** Nombre lisible pour `raisons` (point décimal ; `raisons` n'est pas parsé). */
function n1(n: number): string {
  return Number.isFinite(n) ? n.toFixed(1) : "∞";
}

/** Une caméra inutilisable ne rend aucun classement (plutôt qu'un faux). */
function cameraUtilisable(cam: Camera | undefined | null): cam is Camera {
  if (!cam) return false;
  if (!Array.isArray(cam.viewProjection) || cam.viewProjection.length < 16) return false;
  for (let i = 0; i < 16; i += 1) if (!estFini(cam.viewProjection[i])) return false;
  if (!estFini(cam.largeur) || cam.largeur <= 0) return false;
  if (!estFini(cam.hauteur) || cam.hauteur <= 0) return false;
  return pointFini(cam.position);
}

/**
 * Axe « droite écran » en coordonnées monde.
 *
 * Ce sont les LIGNES 0 et 1 de la matrice vue-projection qui portent la base de
 * la caméra en coordonnées monde : ligne 0 = (droite, −droite·œil), ligne 1 =
 * (haut, −haut·œil), ligne 3 = l'axe optique inversé (d'où `w > 0` devant).
 * C'est exact pour un tronc de cône centré (les termes d'obliquité sont nuls),
 * ce qui est toujours le cas ici. La ligne est ensuite NORMALISÉE : seule sa
 * direction nous intéresse, et l'échelle du tronc n'a pas à entrer dans la
 * mesure d'un décalage (la projection s'en charge).
 */
function droiteEcran(cam: Camera): P3 {
  const m = cam.viewProjection;
  const essais: [number, number, number][] = [
    [m[0], m[4], m[8]],
    [m[1], m[5], m[9]],
    [1, 0, 0],
  ];
  for (const [ax, ay, az] of essais) {
    const len = Math.hypot(ax, ay, az);
    if (estFini(len) && len > 1e-9) return { x: ax / len, y: ay / len, z: az / len };
  }
  return { x: 1, y: 0, z: 0 };
}

/**
 * Empreinte « saturée » : la caméra est si près de la boule (ou dedans) que le
 * point décalé passe DERRIÈRE elle. On ne rend pas `null` pour autant : rendre
 * null ferait DISPARAÎTRE le candidat, alors qu'il occupe toujours l'écran. On
 * sature donc l'empreinte à dix diagonales d'écran — plus grand que n'importe
 * quelle empreinte réelle, ce qui laisse le candidat désignable (pénalisé par
 * `prec` et par `face`) au lieu de le faire taire.
 */
function empreinteSaturee(cam: Camera): number {
  return 10 * Math.hypot(cam.largeur, cam.hauteur);
}

/** Réglages assainis : un réglage absurde ne doit jamais produire NaN. */
function reglagesSurs(r?: Partial<Reglages> | null): Reglages {
  const brut = { ...REGLAGES_DEFAUT, ...(r ?? {}) } as Reglages;
  const D = REGLAGES_DEFAUT;
  const pos = (v: number, def: number) => (estFini(v) && v > 0 ? v : def);
  // Un ratio hors de [0,1] n'est pas « borné » mais REJETÉ : borner 42 à 1
  // disqualifierait silencieusement tout objet non parfaitement opaque, ce qui
  // est exactement le genre de panne muette que ce module doit éviter.
  const ratio = (v: number, def: number) => (estFini(v) && v >= 0 && v <= 1 ? v : def);
  const plancherPx = pos(brut.plancherPx, D.plancherPx);
  return {
    plancherPx,
    facteur: pos(brut.facteur, D.facteur),
    // Le plafond ne peut pas être plus petit que le plancher : sinon
    // `min(plafond, max(plancher, …))` rendrait un rayon de sélection inférieur
    // au plancher promis, en silence.
    plafondPx: Math.max(pos(brut.plafondPx, D.plafondPx), plancherPx),
    seuilOpacite: ratio(brut.seuilOpacite, D.seuilOpacite),
    poidsMinOpacite: ratio(brut.poidsMinOpacite, D.poidsMinOpacite),
    bonusFocal: pos(brut.bonusFocal, D.bonusFocal),
    penaliteHorsEtage: pos(brut.penaliteHorsEtage, D.penaliteHorsEtage),
    penaliteFaceArriere: pos(brut.penaliteFaceArriere, D.penaliteFaceArriere),
    base: pos(brut.base, D.base),
  };
}

// ---------------------------------------------------------------------------
// PROJECTION
// ---------------------------------------------------------------------------

/**
 * Projette un point monde en pixels écran ; rend null s'il est derrière la
 * caméra (w ≤ 0), si la caméra n'est pas exploitable, ou si le résultat n'est
 * pas fini. Convention three.js : matrice colonne-major, ndc.y vers le HAUT,
 * pixels.y vers le BAS — d'où la soustraction à 0,5.
 */
export function projeter(c: Camera, p: { x: number; y: number; z: number }): { x: number; y: number } | null {
  if (!cameraUtilisable(c) || !pointFini(p)) return null;
  const m = c.viewProjection;
  const { x, y, z } = p;
  const w = m[3] * x + m[7] * y + m[11] * z + m[15];
  // `!(w > W_MIN)` est vrai aussi pour NaN : la garde couvre les deux cas.
  if (!(w > W_MIN)) return null;
  const cx = m[0] * x + m[4] * y + m[8] * z + m[12];
  const cy = m[1] * x + m[5] * y + m[9] * z + m[13];
  const px = ((cx / w) * 0.5 + 0.5) * c.largeur;
  const py = (0.5 - (cy / w) * 0.5) * c.hauteur;
  if (!estFini(px) || !estFini(py)) return null;
  return { x: px, y: py };
}

export interface Empreinte {
  /** Centre projeté (px écran). */
  x: number;
  y: number;
  /** Rayon apparent en pixels (0 pour un point : un nœud n'a pas d'étendue). */
  empreinte: number;
}

/**
 * Empreinte apparente (px) d'une boule de rayon `rayon` centrée en `p` — ou
 * `null` si le centre passe derrière la caméra.
 *
 * Hors de l'interface requise, mais exportée : c'est LA grandeur qui manquait
 * pour décider, et la rendre visible permet de la tester et de l'afficher.
 *
 * Deux projections, comme le fait déjà `rayonEcran` (CosmosWeave, ligne 3608),
 * mais le décalage est pris perpendiculairement à l'axe optique : la profondeur
 * est conservée, donc la mesure ne mélange pas taille et éloignement, et elle
 * ne peut pas dégénérer quand on regarde une boule au pôle.
 */
export function empreintePx(
  cam: Camera,
  p: { x: number; y: number; z: number },
  rayon: number,
): Empreinte | null {
  const centre = projeter(cam, p);
  if (!centre) return null;
  const r = estFini(rayon) ? Math.max(0, rayon) : 0;
  if (r === 0) return { x: centre.x, y: centre.y, empreinte: 0 };
  const d = droiteEcran(cam);
  const decale = projeter(cam, { x: p.x + d.x * r, y: p.y + d.y * r, z: p.z + d.z * r });
  if (!decale) return { x: centre.x, y: centre.y, empreinte: empreinteSaturee(cam) };
  const e = Math.hypot(decale.x - centre.x, decale.y - centre.y);
  if (!estFini(e)) return { x: centre.x, y: centre.y, empreinte: empreinteSaturee(cam) };
  return { x: centre.x, y: centre.y, empreinte: e };
}

// ---------------------------------------------------------------------------
// CLASSEMENT
// ---------------------------------------------------------------------------

/**
 * Classe les candidats pour un pointeur en pixels écran. Rendu TRIÉ, meilleur
 * d'abord, et il contient TOUS les candidats exploitables — y compris ceux qui
 * ne sont pas désignables (`score = 0`), avec la raison du refus : c'est ce qui
 * permet de répondre à « pourquoi ce clic n'a rien fait ? » sans navigateur.
 *
 * Tri : score décroissant, puis focal, puis px croissant, puis rayon de
 * sélection croissant. À égalité complète, l'ordre d'entrée est conservé (le
 * tri de `Array#sort` est stable depuis ES2019) : le résultat ne dépend donc
 * pas du moteur.
 */
export function classer(
  candidats: Candidat[],
  cam: Camera,
  pointeur: { x: number; y: number },
  etage: number,
  reglages?: Partial<Reglages>,
): Classement[] {
  const R = reglagesSurs(reglages);
  const sortie: Classement[] = [];
  if (!Array.isArray(candidats) || candidats.length === 0) return sortie;
  if (!cameraUtilisable(cam)) return sortie;
  if (!pointeur || !estFini(pointeur.x) || !estFini(pointeur.y)) return sortie;

  for (const c of candidats) {
    // ── VALIDITÉ ── Un candidat non fini n'existe pas pour le sélecteur.
    if (!c || !pointFini(c.ancre)) continue;
    if (!estFini(c.rayon) || !estFini(c.opacite)) continue;
    const rayon = Math.max(0, c.rayon); // un rayon négatif n'a pas de sens
    const opacite = borne(c.opacite, 0, 1);
    const raisons: string[] = [];

    // ── EMPREINTE ── Tout part de là : la taille vue, pas la taille déclarée.
    const emp = empreintePx(cam, c.ancre, rayon);
    if (!emp) {
      raisons.push("ancre DERRIÈRE la caméra : aucune position écran, donc aucune mesure en px → NON DÉSIGNABLE (score 0)");
      sortie.push({ candidat: c, px: Infinity, rayonPx: R.plancherPx, score: 0, raisons });
      continue;
    }

    const distAncre = Math.hypot(pointeur.x - emp.x, pointeur.y - emp.y);
    const px = Math.max(0, distAncre - emp.empreinte);
    const rayonPx = Math.min(R.plafondPx, Math.max(R.plancherPx, emp.empreinte * R.facteur));
    const portee = emp.empreinte + rayonPx;
    const dansPortee = distAncre <= portee;

    // ── FACTEURS ── (aucun dénominateur ne peut être nul : portee ≥ 6 px)
    const proxi = portee / (portee + distAncre);
    const prec = R.plafondPx / (R.plafondPx + emp.empreinte);
    const vis = R.poidsMinOpacite + (1 - R.poidsMinOpacite) * opacite;
    const dansEtage = Array.isArray(c.etages) && c.etages.indexOf(etage) >= 0;
    const focal = c.focal === true;
    const facteurEtage = dansEtage ? (focal ? R.bonusFocal : 1) : R.penaliteHorsEtage;
    const dCam = distMonde(cam.position, c.ancre);
    const dedansBoule = rayon > 0 && dCam <= rayon;
    const facteurFace = dedansBoule ? R.penaliteFaceArriere : 1;

    raisons.push(
      `empreinte ${n1(emp.empreinte)} px (rayon monde ${n1(rayon)}) → rayon de sélection ${n1(rayonPx)} px `
      + `[plancher ${n1(R.plancherPx)}, ×${R.facteur}, plafond ${n1(R.plafondPx)}]`,
    );
    raisons.push(
      `pointeur à ${n1(distAncre)} px de l'ancre et ${n1(px)} px de l'empreinte → proximité ×${proxi.toFixed(3)} `
      + `(portée ${n1(portee)} px)`,
    );
    raisons.push(`précision ×${prec.toFixed(3)} (empreinte ${n1(emp.empreinte)} px contre plafond ${n1(R.plafondPx)} px)`);
    raisons.push(`opacité ${opacite.toFixed(2)} → visibilité ×${vis.toFixed(3)}`);
    raisons.push(
      dansEtage
        ? `étage ${etage} légitime${focal ? " et focal" : " mais NON focal"} → ×${facteurEtage.toFixed(2)}`
        : `étage ${etage} hors de [${(c.etages ?? []).join(", ")}] → repli ×${facteurEtage.toFixed(2)}`,
    );
    if (dedansBoule) {
      raisons.push(
        `caméra DANS la boule (${n1(dCam)} ≤ ${n1(rayon)}) : seule la face arrière serait visible → `
        + `×${R.penaliteFaceArriere.toFixed(2)}`,
      );
    }

    // ── VETOS ── Un score nul n'arrive jamais par accident : il est motivé.
    const vetos: string[] = [];
    if (opacite < R.seuilOpacite) {
      vetos.push(`opacité ${opacite.toFixed(2)} < seuil ${R.seuilOpacite.toFixed(2)} → objet effacé`);
    }
    if (!dansPortee) {
      vetos.push(`hors de portée : ${n1(distAncre)} px > ${n1(portee)} px`);
    }

    if (vetos.length) {
      raisons.push(...vetos, "→ NON DÉSIGNABLE (score 0)");
      sortie.push({ candidat: c, px, rayonPx, score: 0, raisons });
      continue;
    }

    const score = R.base * proxi * prec * vis * facteurEtage * facteurFace;
    raisons.push(
      `score ${n1(score)} = ${R.base} × ${proxi.toFixed(3)} × ${prec.toFixed(3)} × ${vis.toFixed(3)} `
      + `× ${facteurEtage.toFixed(2)} × ${facteurFace.toFixed(2)}`,
    );
    sortie.push({ candidat: c, px, rayonPx, score, raisons });
  }

  sortie.sort((a, b) => {
    if (b.score !== a.score) return b.score - a.score;
    const fa = a.candidat.focal ? 1 : 0;
    const fb = b.candidat.focal ? 1 : 0;
    if (fb !== fa) return fb - fa;
    if (a.px !== b.px) return a.px - b.px;
    if (a.rayonPx !== b.rayonPx) return a.rayonPx - b.rayonPx;
    return 0; // stabilité : l'ordre d'entrée tranche
  });

  return sortie;
}

/**
 * Décision : le meilleur candidat DÉSIGNABLE, ou `null` si rien n'est
 * désignable. Aucun repli « au plus proche » : un clic dans le vide ne doit
 * rien choisir, il ne doit pas choisir le voisin le plus lointain.
 */
export function choisir(
  candidats: Candidat[],
  cam: Camera,
  pointeur: { x: number; y: number },
  etage: number,
  reglages?: Partial<Reglages>,
): Classement | null {
  for (const cl of classer(candidats, cam, pointeur, etage, reglages)) {
    if (cl.score > 0) return cl; // trié : le premier désignable est le meilleur
  }
  return null;
}
