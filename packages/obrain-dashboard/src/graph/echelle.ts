/**
 * ── LA LOI D'ÉCHELLE, SORTIE DU COMPOSANT POUR ÊTRE TESTÉE ──────────────────
 *
 * Module PUR (aucun import, aucun accès au DOM), comme `densite.ts` et
 * `fractale.ts` : un calcul qui décide de l'affichage se teste sans navigateur.
 *
 * POURQUOI CE MODULE EXISTE. Ces cinq fonctions et six constantes vivaient DANS
 * `CosmosWeave.tsx` (7 000 lignes), donc n'étaient testables nulle part. Or c'est
 * exactement la loi qui a échoué le 2026-09-15 : les quatre crans ne faisaient
 * rien, et la cause était un cran qui VISait un monde pendant que le palier en
 * MESURAIT un autre — deux autorités pour un même fait. Un test de la loi
 * elle-même (bandes, aller-retour, monotonie) aurait dit lequel des deux mentait,
 * en une seconde, sans navigateur.
 *
 * ── LA LOI, ET CE QU'ELLE GARANTIT ──────────────────────────────────────────
 *
 *  1. `t` (la « descente », 0 → 1) et la DISTANCE sont deux écritures de la même
 *     grandeur, reliées par une échelle LOGARITHMIQUE : `distancePourT`. Chaque
 *     doublement de distance prend la même part de course, comme sur une carte.
 *  2. L'échelle est exprimée en RAYONS du monde (`loin = 14 R`, `pres = 0,25 R`) :
 *     elle vaut donc pour un monde de 30 unités comme pour un de 1 500.
 *  3. Les quatre paliers sont des RÉGIONS de cette échelle, pas des constantes
 *     posées côte à côte : deux seuils de `t` (`BANDE_MONDE`, `BANDE_DEDANS`)
 *     puis `pres` (`PALIER_DEDANS_SIGNAUX`), et l'espace des signaux à `PALIER_SIGNAUX`.
 *  4. Un cran (bouton de vue) VISE une valeur de `t` (`CRAN_T`) — sauf le cran
 *     des signaux, qui vise une distance, `t` saturant à 1 sous `pres`.
 *
 * L'INVARIANT QUI COMPTE, et qui est testé : `palierDe(distancePourPalier(n, R), R)`
 * vaut `n`. Autrement dit **un cran qui vise sa bande la retrouve** — viser et
 * mesurer parlent alors de la même longueur, ce qui n'était plus vrai quand les
 * deux parlaient de mondes différents.
 */

/** Seuil de `t` sous lequel on est « dans le monde » (au-delà : l'espace). */
export const BANDE_MONDE = 0.34;
/** Seuil de `t` au-delà duquel on est « dedans » (sous : la bande « monde »). */
export const BANDE_DEDANS = 0.72;
/** Valeur de descente visée par chaque cran de bouton de vue. */
export const CRAN_T = { monde: 0.45, groupe: 0.85 } as const;
/** `pres` du curseur : « dedans ». */
export const PALIER_DEDANS_SIGNAUX = 0.25;
/** `presS` du curseur : l'espace des signaux (le fond de l'échelle). */
export const PALIER_SIGNAUX = 0.08;
/** Les quatre crans, dans l'ordre de la descente. */
export const PALIERS = ["espace", "monde", "groupe", "signaux"] as const;

/** Un palier d'échelle : `0` espace · `1` monde · `2` groupe · `3` signaux. */
export type Palier = 0 | 1 | 2 | 3;

/**
 * Rayon utilisable. Un monde de rayon nul ou négatif n'a pas d'échelle : sans ce
 * garde, `distancePourT` rendrait `NaN` (logarithme de zéro) et `palierDe`
 * répondrait « groupe » pour toute distance — un cran faux, silencieusement.
 */
const rayon = (R: number) => (Number.isFinite(R) && R > 0 ? R : 30);

/** Distance caméra qui DONNE la valeur `t` voulue, pour un monde de rayon `R`. */
export function distancePourT(t: number, R: number): number {
  const r = rayon(R);
  const loin = r * 14, pres = r * 0.25;
  // `t` est BORNÉ : au-delà de 1 le curseur sature (voir le quatrième palier),
  // et sous 0 la caméra serait derrière le centre.
  const tc = Math.max(0, Math.min(1, Number.isFinite(t) ? t : 0));
  return loin * Math.exp(-tc * Math.log(loin / pres));
}

/** Où en est-on sur l'échelle, à partir de la distance au centre et du rayon. */
export function palierDe(d: number, R: number): Palier {
  const r = rayon(R);
  if (!Number.isFinite(d)) return 0;               // distance inconnue : on ne prétend rien
  const dd = Math.max(0, d);
  if (dd > distancePourT(BANDE_MONDE, r)) return 0;
  if (dd > distancePourT(BANDE_DEDANS, r)) return 1;
  if (dd > r * PALIER_DEDANS_SIGNAUX) return 2;
  return 3;
}

/**
 * Distance caméra qui DONNE le palier `n` pour un monde de rayon `R`.
 *
 * Les crans 1 et 2 visent une VALEUR DE `t` (leur bande) ; le cran 0 est la
 * distance d'ensemble (14 R) et le cran 3 l'espace des signaux (0,08 R), parce
 * que `t` y sature à 1 et ne peut plus rien distinguer.
 */
export function distancePourPalier(n: Palier, R: number): number {
  const r = rayon(R);
  return n === 0 ? r * 14
    : n === 1 ? distancePourT(CRAN_T.monde, r)
      : n === 2 ? distancePourT(CRAN_T.groupe, r)
        : r * PALIER_SIGNAUX;
}

/** Le libellé d'un palier — écrit UNE fois, pour que le mot et le cran s'accordent. */
export const nomPalier = (p: Palier): string => PALIERS[p] ?? PALIERS[0];

/**
 * ── LA BANDE D'UN PALIER : [intérieur, extérieur] ───────────────────────────
 *
 * Mesuré (2026-09-16) : les crâns « groupe » et « signaux » visent JUSTE (11,07
 * et 1,94 pour un monde de rayon 24,2 — chacun dans sa bande), et la caméra se
 * pose pourtant à ~39-42, donc DANS LA BANDE « MONDE ». Un cran qui vise sa bande
 * et se pose ailleurs n'est pas un cran : il faut pouvoir dire, sans navigateur,
 * où commence et où finit chaque bande.
 *
 * Convention, celle du code et donc testée ici : les comparaisons sont STRICTES,
 * donc la distance qui vaut exactement un seuil appartient à la bande INTÉRIEURE.
 * La première borne d'une bande est donc exclue, la seconde incluse.
 */
export function bandeDe(n: Palier, R: number): { dedans: number; dehors: number } {
  const r = rayon(R);
  const seuilMonde = distancePourT(BANDE_MONDE, r);   // premier seuil rencontré
  const seuilDedans = distancePourT(BANDE_DEDANS, r);
  const pres = r * PALIER_DEDANS_SIGNAUX;
  if (n === 0) return { dedans: seuilMonde, dehors: Infinity };
  if (n === 1) return { dedans: seuilDedans, dehors: seuilMonde };
  if (n === 2) return { dedans: pres, dehors: seuilDedans };
  return { dedans: 0, dehors: pres };
}

/**
 * Le MILIEU géométrique d'une bande — l'endroit où un cran se pose sans être à la
 * merci du dernier chiffre flottant. Géométrique, et non arithmétique : l'échelle
 * est logarithmique, donc c'est la moyenne des LOGARITHMES qui tombe au milieu.
 */
export function milieuBande(n: Palier, R: number): number {
  const r = rayon(R);
  if (n === 0) return distancePourT(BANDE_MONDE, r) * 2;   // « tout voir » : deux fois le seuil
  if (n === 3) return r * PALIER_SIGNAUX;                  // le fond de l'échelle
  const { dedans, dehors } = bandeDe(n, r);
  return Math.sqrt(dedans * dehors);
}

/**
 * ── LA DISTANCE À VISER POUR UN CRAN, GARANTIE DANS SA BANDE ────────────────
 *
 * C'est le correctif que les coins ont rendu possible : la visée n'est plus un
 * `distancePourPalier` qu'on espère, mais une distance dont on VÉRIFIE qu'elle
 * tombe dans la bande demandée — et qu'on corrige au milieu de la bande sinon.
 * L'invariant est testé : `palierDe(viserCran(n, R), R) === n` pour tous les
 * crans et tous les rayons, y compris dégénérés.
 *
 * `dActuelle` (optionnel) sert la règle « aucun cran ne recule POUR ALLER PLUS
 * PROFOND » : si l'on demande un cran plus profond que celui où l'on est déjà,
 * on ne vise jamais plus loin que la distance courante — sinon le geste éloigne
 * la caméra au lieu de l'enfoncer, ce que ce fichier a déjà corrigé une fois
 * pour le cran « groupe » (« un cran qui recule n'est pas un cran »).
 */
export function viserCran(n: Palier, R: number, dActuelle?: number): number {
  const r = rayon(R);
  const vise = distancePourPalier(n, r);
  let d = palierDe(vise, r) === n ? vise : milieuBande(n, r);
  if (typeof dActuelle === "number" && Number.isFinite(dActuelle) && dActuelle > 0) {
    if (palierDe(dActuelle, r) < n) {
      // On va plus profond : la cible ne doit pas être PLUS LOIN que l'actuelle.
      d = Math.min(d, dActuelle);
    }
  }
  // Dernier garde : la bande demandée, quoi qu'il arrive.
  return palierDe(d, r) === n ? d : milieuBande(n, r);
}

