import * as THREE from "three";
import { fetchModuleArcs, fetchModules, fetchSpace, fetchSpacePage } from "../lib/api";
import { PAGE } from "./densite";

/**
 * ── LA LOI D'ÉCHELLE, ÉCRITE UNE FOIS ────────────────────────────────────────
 *
 * DÉFAUT MESURÉ (2026-09-14, service 8432, store `runtime-dev.obram`, 427 530
 * nœuds) : dans l'Atlas — la face par DÉFAUT — un clic sur une bulle de base
 * n'émettait AUCUNE requête d'exploration. Sonde CDP : sept
 * `/api/viz/activation`, zéro `/api/viz/modules`, zéro `/api/viz/space`, zéro
 * `/api/viz/subgraph`. La cause est dans `StageContinu.repliCentre` : le centre
 * de repli est fabriqué à partir du LIBELLÉ du store (`tcp_v4_rcv` pour
 * « complet », `e1000_probe` pour « linux »…), et pour « Runtime dev (engrams) »
 * il renvoie la chaîne VIDE — donc `chargerSub` sortait immédiatement. On
 * entrait dans un monde sans rien y révéler : la même boule de points, à la
 * même place.
 *
 * Et là où un sous-graphe était chargé (Linux e1000), la disposition était une
 * SPHÈRE DE FIBONACCI (`Math.PI * (3 - Math.sqrt(5))`) : une boule de points
 * uniforme, sans aucune organisation — l'échelle suivante n'existait pas.
 *
 * Ce module porte la loi qui manquait, et rien d'autre : la descente est
 * RÉCURSIVE, chaque niveau se déduit du même calcul, et le serveur en est la
 * seule autorité sur ce que contient un niveau.
 *
 * ── LA LOI ──────────────────────────────────────────────────────────────────
 *  · `rang(profondeur) = rangRacine + profondeur` — c'est la convention déjà
 *    tenue par `StoreOverview` (`fetchSpace(store, prefixNow, rank)`), et elle
 *    vient du serveur : `/api/viz/space` agrège `split(qn,'.')[rank]` ;
 *  · `prefixe(profondeur) = prefixe(parent) + "." + nom` — le sous-espace est
 *    un CHEMIN de tokens, donc la descente est un chemin, pas un index ;
 *  · le RAYON d'un groupe suit la racine CUBIQUE de sa part : le VOLUME — seul
 *    canal qui se lise dans une scène 3D — est alors proportionnel au nombre
 *    réel de nœuds. Un plancher et un plafond bornent la lisibilité, et sont
 *    les seuls écarts assumés ;
 *  · l'OUVERTURE d'un niveau est une TAILLE APPARENTE, pas une distance : la
 *    même formule (2r / (2·d·tan(fov/2))) ouvre donc les niveaux à la même
 *    échelle visuelle, quelle que soit la taille absolue du monde traversé
 *    (17 000 nœuds ou 1 500 000) ;
 *  · l'HYSTÉRÉSIS (ouverture à un seuil, fermeture à un seuil plus bas) est ce
 *    qui empêche l'oscillation mesurée sur la descente du Cosmos (48 % → 100 %
 *    → 60 % → 99 % … soit 20 sauts de plus de 15 points sur 39 pas).
 */
export const FRACTALE = {
  /** Garde-fou : au-delà, un chemin de qn n'a plus de token (le serveur rend
   *  un nom vide) et la descente n'a plus de sens. */
  profondeurMax: 12,
  /** Taille apparente qui OUVRE un groupe (mesurée sur la descente existante
   *  du Cosmos : la rampe d'ouverture y travaille entre 0,12 et 0,37). */
  seuilOuverture: 0.30,
  /** Taille apparente sous laquelle on REFERME : plus basse, pour que la
   *  fermeture ne rejoue pas l'ouverture (hystérésis). */
  seuilGarde: 0.16,
  /**
   * ── LES GROUPES D'UN NIVEAU : UNE PAGE, PAS UN PLAFOND ────────────────────
   *
   * Ce réglage s'appelait `groupesMax` et valait 24 — un PLAFOND, appliqué
   * AVANT que quiconque lise la longueur de la liste. C'était un plafond
   * SILENCIEUX, et le pire des deux mondes : `/api/viz/space` sert jusqu'à 200
   * ensembles, le panneau possédait la phrase « N sur total », et ni l'un ni
   * l'autre ne pouvait se voir — `n` et `total` valaient 24 tous les deux.
   *
   * C'est désormais une PAGE (`densite.ts` : `PAGE.pas`), et le reste de la
   * liste est CONSERVÉ sur l'étage : densifier ne redemande donc rien au
   * serveur, il dessine la page suivante. Un plafond qui ne se distingue pas
   * d'une donnée absente est un mensonge ; une page qui s'annonce est une
   * mesure.
   */
  pasPage: PAGE.pas,
  /** Un groupe ne dépasse jamais cette part du rayon de son parent… */
  partMaxGroupe: 0.55,
  /** …et ne descend pas sous ce plancher (sinon il n'est plus désignable). */
  plancherGroupe: 0.08,
  /** Matière dessinée par niveau (toutes boules confondues). */
  budgetParticules: 2600,
  /** En dessous de ce nombre de nœuds, un groupe n'a plus de sous-groupes
   *  exploitables : on montre ses NŒUDS (le fond de l'échelle). */
  seuilFeuille: 60,
} as const;

export interface GroupeFractal {
  nom: string;
  count: number;
}

/** Un étage de la descente : le chemin, le rang, et ce que le niveau contient. */
export interface EtageFractal {
  /** `world` (racine) puis les noms de groupes, joints par « · ». */
  chemin: string;
  /** Préfixe de qn du sous-espace (vide = racine du store). */
  prefixe: string;
  /** Rang du token agrégé à ce niveau (`split(qn,'.')[rang]`). */
  rang: number;
  nom: string;
  count: number;
  profondeur: number;
}

/** Le préfixe d'un enfant : les tokens sont joints par « . » (forme des qn). */
export const cheminEnfant = (prefixe: string, nom: string) =>
  prefixe ? `${prefixe}.${nom}` : nom;

/**
 * Rang de la racine d'un store — DEMANDÉ au serveur, jamais supposé.
 *
 * `/api/viz/modules` choisit son rang de découpage selon la profondeur réelle
 * des qn du store (mesuré : 3 pour `runtime-dev`, 2 pour `linux-full3`) ; le
 * front n'a aucune raison de le deviner, et le deviner fausserait toute la
 * descente (le premier niveau ne serait pas celui des modules).
 */
export async function rangRacine(store: string, signal?: AbortSignal): Promise<number> {
  const m = await fetchModules(store, signal).catch(() => ({ modules: [], rank: undefined }));
  return typeof m.rank === "number" && m.rank > 0 ? m.rank : 2;
}

/**
 * Un nom acceptable comme TOKEN de sous-espace.
 *
 * Deux raisons, toutes deux mesurées :
 *  · `/api/viz/space` REFUSE un préfixe qui sort de `[A-Za-z0-9._@+%-]` et
 *    répond 400 — descendre dans un « groupe » nommé `0/lib/node_modules/…`
 *    échouerait donc à coup sûr (ces noms viennent de qn dont le chemin
 *    contient des points : `file:/home/…/v22.19.0/lib/…` → `split('.')[3]`) ;
 *  · un effectif de 1 n'est pas un groupe : c'est un item, et il ne se
 *    décompose pas. Mesuré sur `runtime-prod` : sans ces deux filtres, l'étage
 *    1 montrait 24 « groupes » dont 18 de un nœud, qui masquaient les six
 *    vraies racines du dépôt (`deepseek_harness`, `banc_obrain`, `aletheia`…).
 */
export const tokenValide = (nom: string) =>
  !!nom && nom.length <= 400 && /^[A-Za-z0-9._@+%-]+$/.test(nom);

/** Ce qu'un niveau CONTIENT, et ce qu'on en DESSINE.
 *
 * `tous` est la matière reçue (bornée par le plafond de l'appel) ; `montres` en
 * est la page courante. Les deux sont conservés ENSEMBLE parce que c'est la
 * seule façon de savoir qu'il reste quelque chose : densifier est alors un
 * redessin (aucun appel), et `total > montres` est la preuve qu'un plafond n'a
 * pas mangé la donnée.
 */
export interface NiveauFractal {
  /** Tout ce que l'appel a rendu, filtré et trié — la matière du niveau. */
  tous: GroupeFractal[];
  /** La page courante (`tous.slice(0, page)`). */
  montres: GroupeFractal[];
  /** Nombre de groupes de la page courante. */
  page: number;
  /** Nombre de groupes réellement reçus (`tous.length`). */
  total: number;
  /**
   * ── CE QUE LE SERVEUR A COMPTÉ, AVANT NOS FILTRES ────────────────────────
   *
   * `SKIP` du serveur porte sur des LIGNES BRUTES, pas sur les groupes que nous
   * gardons : filtrer (`tokenValide`, `count > 1`) décale donc l'offset si on le
   * calcule sur `total`. Mesuré sur `runtime-dev` (`deepseek_harness`, rang 4) :
   * 200 lignes brutes rendues, dont un groupe au nom vide de 8 438 nœuds écarté —
   * demander la page suivante à `offset = total` (filtré) ramènerait des lignes
   * déjà connues, et n'avancerait que de la différence. C'est ce nombre-ci qu'on
   * additionne pour paginer.
   */
  bruts: number;
  /** L'appel a été coupé par le plafond du SERVEUR : la suite est chez lui. */
  plafondAtteint: boolean;
  /** Rang du token agrégé à ce niveau (rendu par le serveur). */
  rang: number;
}

/** Enfants d'un étage — la descente d'UN cran, et elle seule.
 *
 * `links = false` N'EST PAS UN DÉTAIL : mesuré sur ce store, l'agrégat SANS
 * liens coûte 62 ms à chaud, et le MÊME appel avec `links=1` en coûte 13 874 —
 * 224 fois plus, parce qu'il agrège les arcs CALLS entre enfants (avec les
 * liaisons non dirigées, c'est le poste dominant). Pendant une descente, on
 * paie donc l'agrégat léger, et les arcs restent pour l'étage qu'on REGARDE.
 *
 * ── CE QUI A CHANGÉ (densifier ≠ approfondir) ──────────────────────────────
 * La fonction rendait une liste DÉJÀ coupée à 24, et son appelant lisait la
 * longueur DE CETTE COUPE : le panneau annonçait « 24 ensembles » là où le
 * serveur en avait servi 200, et rien ne pouvait le démentir. Elle rend
 * maintenant la liste ENTIÈRE (`tous`) et la page à dessiner (`montres`) :
 * le plafond devient visible, et la page suivante ne coûte aucun appel.
 */
export async function enfantsDe(
  store: string,
  prefixe: string,
  rang: number,
  page: number = PAGE.pas,
  signal?: AbortSignal,
): Promise<NiveauFractal> {
  const r = await fetchSpace(store, prefixe, rang, PAGE.plafondAppel, signal, false);
  return niveauDe(r, page, rang);
}

/** La MÊME loi de niveau, appliquée à une page déjà reçue (voir `pageServeur`). */
function niveauDe(
  r: { ensembles?: { name: string; count: number }[]; rank?: number; tronque?: boolean },
  page: number,
  rang: number,
): NiveauFractal {
  const bruts = r.ensembles || [];
  const tous: GroupeFractal[] = bruts
    .filter((e) => e && tokenValide(String(e.name)) && e.count > 1)
    .sort((a, b) => b.count - a.count)
    .map((e) => ({ nom: e.name, count: e.count }));
  const n = Math.min(tous.length, Math.max(0, Math.floor(page) || 0));
  return {
    tous,
    montres: tous.slice(0, n),
    page: n,
    total: tous.length,
    bruts: bruts.length,
    // Le plafond se lit d'abord sur ce que le SERVEUR DIT (`tronque`, page
    // pleine) ; à défaut on le mesure (200 rendus = il peut y en avoir d'autres).
    // Mesurer sur les bruts, avant nos filtres : un groupe d'un nœud écarté ne
    // dit rien du plafond du serveur.
    plafondAtteint: typeof r.tronque === "boolean"
      ? r.tronque
      : bruts.length >= PAGE.plafondAppel,
    rang: r.rank ?? rang,
  };
}

/**
 * ── LA PAGE SUIVANTE, CHEZ LE SERVEUR (densifier au-delà de son plafond) ────
 *
 * Tant que la matière reçue n'est pas toute dessinée, densifier est un REDESSIN
 * (aucun appel). Mais quand la page servie est PLEINE (`tronque`), le reste est
 * chez le serveur : densifier devient une requête, et c'est la même question
 * posée au même niveau — `offset`, pas un cran de plus. `offset` est l'index du
 * premier ensemble de la page demandée.
 */
export async function pageServeur(
  store: string,
  prefixe: string,
  rang: number,
  offset: number,
  signal?: AbortSignal,
): Promise<NiveauFractal> {
  const r = await fetchSpacePage(store, {
    prefix: prefixe, rank: rang, top: PAGE.plafondAppel, offset, links: false, signal,
  });
  // La page du serveur est ENTIÈRE : on la rend telle quelle (`page` = tout),
  // c'est l'appelant qui décide combien il en dessine.
  const niveau = niveauDe(r, PAGE.plafondAppel, rang);
  return { ...niveau, montres: niveau.tous, page: niveau.tous.length };
}

/**
 * Vrai si la réponse du serveur annonce une suite (page pleine).
 *
 * ── CE QUE LE SERVEUR DIT, PAS CE QUE NOUS GARDONS ────────────────────────
 * Cette fonction exigeait `total >= 200` en plus de `plafondAtteint`, et les deux
 * ne parlent pas de la même chose : `plafondAtteint` vient du serveur (`tronque`),
 * `total` est ce qui SURVIT à nos filtres. Un niveau tronqué dont le filtre écarte
 * ne serait-ce qu'une ligne n'armait donc plus la pagination — mesuré sur
 * `runtime-dev` : 200 lignes rendues, un nom vide de 8 438 nœuds écarté, et la page
 * suivante du serveur devenue inatteignable. Un seul juge : ce que le serveur dit.
 */
export const suiteChezLeServeur = (n: NiveauFractal) => n.plafondAtteint;

/** Vrai si le geste de densification doit demander la page suivante AU SERVEUR.
 *  Ici `page` est le NOMBRE dessiné, et `montres` la liste qui l'accompagne
 *  (`densite.ts` nomme `montres` le nombre : c'est la même grandeur, vue de
 *  l'étage ou vue de la loi). */
export const densifierParAppel = (n: NiveauFractal) =>
  suiteChezLeServeur(n) && n.page >= n.total;

/** Arcs réels entre les enfants d'un DÉJÀ-OUVERT (appel lourd : à la demande). */
export const arcsDe = (store: string, top = 16, limit = 60, signal?: AbortSignal) =>
  fetchModuleArcs(store, top, limit, signal).catch(() => []);

/**
 * Un niveau est-il une FEUILLE ?
 *
 * Deux cas, et ils sont différents :
 *  · moins de deux enfants → il n'y a rien à organiser : ce qui reste, ce sont
 *    les NŒUDS. C'est le fond de l'échelle ;
 *  · un effectif trop faible → les enfants existent mais ne portent plus rien
 *    de lisible (`linux-full3` rend des tokens vides).
 * Dans les deux cas on descend aux nœuds réels, groupés par type — c'est là
 * que « la boule de points s'organise par groupe ».
 */
export function estFeuille(groupes: GroupeFractal[], count: number): boolean {
  return groupes.length < 2 || count <= FRACTALE.seuilFeuille;
}

/**
 * Rayons des enfants — la part de VOLUME dit la part RÉELLE.
 *
 * Normaliser par le plus gros enfant (`count / max`) est la forme déguisée de
 * l'erreur déjà corrigée dans le Cosmos : deux niveaux de mêmes PROPORTIONS
 * mais de tailles différentes rendraient exactement les mêmes rayons, et un
 * groupe 100 fois plus lourd n'aurait que 2,8 fois le rayon. On normalise donc
 * par le TOTAL du niveau (`count / total`), dont la racine cubique est la part
 * de rayon qui rend le volume proportionnel à l'effectif.
 */
export function rayons(groupes: GroupeFractal[], rayonParent: number): number[] {
  const total = Math.max(1, groupes.reduce((a, g) => a + g.count, 0));
  return groupes.map((g) => {
    const part = Math.cbrt(g.count / total);
    return rayonParent * Math.min(
      FRACTALE.partMaxGroupe,
      Math.max(FRACTALE.plancherGroupe, FRACTALE.partMaxGroupe * part),
    );
  });
}

/**
 * Directions des enfants : DÉTERMINISTES, jamais tirées au hasard.
 *
 * Une disposition aléatoire change à chaque rechargement — on ne peut alors ni
 * revenir à un groupe « là où il était », ni comparer deux niveaux. L'angle
 * d'or (2,399…) et le sinus doré répartissent les directions sans amas ni
 * alignement, sans aucun tirage.
 */
export function directions(n: number): THREE.Vector3[] {
  const out: THREE.Vector3[] = [];
  for (let i = 0; i < n; i++) {
    const a = i * 2.399963229728653;
    const y = (i / Math.max(1, n - 1)) * 1.6 - 0.8;
    const r = Math.sqrt(Math.max(0, 1 - Math.min(1, y * y * 0.62)));
    out.push(new THREE.Vector3(Math.cos(a) * r, Math.sin(i * 1.618) * 0.45, Math.sin(a) * r).normalize());
  }
  return out;
}

/**
 * Taille APPARENTE d'une sphère de rayon `rayon` vue à `distance`.
 *
 * C'est la grandeur qui pilote l'ouverture des niveaux, et elle seule : elle
 * vaut hauteur de l'objet / hauteur du champ visible, donc ~0,5 quand l'objet
 * remplit la moitié de l'écran. Un seuil sur cette grandeur ouvre « au même
 * moment visuel » un monde de rayon 20 et un monde de rayon 1 500.
 */
export function tailleApparente(rayon: number, distance: number, fovDeg: number, aspect: number): number {
  const dist = Math.max(1e-3, distance);
  const haut = (2 * rayon) / (2 * dist * Math.tan((fovDeg * Math.PI) / 360));
  return Math.min(haut, haut / Math.max(0.2, aspect));
}

/** Part de matière d'un groupe dans le budget du niveau (prorata des effectifs). */
export function particules(count: number, total: number, budget = FRACTALE.budgetParticules): number {
  const part = count / Math.max(1, total);
  return Math.max(24, Math.round(budget * part));
}

/**
 * Nuée de points d'un groupe : un VOLUME plein (et non une coquille), à
 * direction déterministe. Le rayon de chaque point suit la racine cubique du
 * tirage — sans quoi la nuée serait deux fois plus dense au centre qu'à la
 * périphérie, ce qui se lit comme un cœur brillant qui n'existe pas.
 */
export function nuee(rayon: number, nb: number, teinte: THREE.Color): THREE.Points {
  const pos = new Float32Array(nb * 3);
  const col = new Float32Array(nb * 3);
  for (let k = 0; k < nb; k++) {
    const a = Math.random() * Math.PI * 2;
    const ph = Math.acos(2 * Math.random() - 1);
    const rr = rayon * Math.cbrt(Math.random());
    pos[k * 3] = rr * Math.sin(ph) * Math.cos(a);
    pos[k * 3 + 1] = rr * Math.cos(ph);
    pos[k * 3 + 2] = rr * Math.sin(ph) * Math.sin(a);
    col[k * 3] = teinte.r; col[k * 3 + 1] = teinte.g; col[k * 3 + 2] = teinte.b;
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute("position", new THREE.BufferAttribute(pos, 3));
  g.setAttribute("color", new THREE.BufferAttribute(col, 3));
  const m = new THREE.PointsMaterial({
    size: 1.5, vertexColors: true, sizeAttenuation: true,
    transparent: true, opacity: 0.95, depthWrite: false,
  });
  return new THREE.Points(g, m);
}
