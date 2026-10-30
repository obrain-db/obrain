/**
 * ── DÉCOUPAGE D'UNE RÉPONSE AUTOUR DE SES APPELS D'OUTILS ────────────────────
 *
 * Module PUR (aucun import de React, aucun accès au DOM) : il est testable tel
 * quel, et `segments.test.mjs` le fait. C'est la pratique du dépôt pour les
 * primitives — un calcul qui décide de l'affichage se teste sans navigateur.
 *
 * LE PROBLÈME QU'IL RÉSOUT. Un appel d'outil n'est pas un journal à part : c'est
 * ce que le modèle a FAIT pour répondre, et il a eu lieu à un endroit précis de
 * la réponse. Le serveur envoie donc, avec chaque appel, `pos` = la longueur du
 * texte DÉJÀ écrit à cet instant (trame `outil`). Ce module rend la réponse en
 * morceaux, l'appel venant ENTRE deux morceaux.
 */

/** Un appel d'outil : ce que l'engine a demandé à la base, et ce qu'il a reçu. */
export interface OutilAppel {
  nom: string;
  arguments: string;
  /** Lignes RÉELLEMENT livrées au modèle (pas celles qui étaient offertes). */
  lignes: number;
  ms: number;
  monde: string;
  /** Le contenu a été coupé au plafond : `lignes` est alors un plafond. */
  tronque?: boolean;
  erreur?: string | null;
  /**
   * Longueur du TEXTE DÉJÀ ÉCRIT quand cet appel a eu lieu (le serveur l'envoie
   * dans la trame `outil`). Absent sur les tours enregistrés avant ce champ : on
   * retombe alors sur la fin du texte, seul endroit qui ne mente pas sur ce
   * qu'on sait d'eux.
   */
  pos?: number;
}

/** Un morceau de réponse : du texte, ou un appel d'outil à sa place. */
export type Segment = { texte: string } | { outil: OutilAppel };

/**
 * Découpe `texte` aux endroits où les appels ont eu lieu.
 *
 * DEUX PRÉCAUTIONS, chacune née d'un défaut d'affichage possible :
 *  1. on n'insère jamais au milieu d'un bloc Markdown — la position est ramenée
 *     à la dernière ligne VIDE qui la précède, sinon un tableau ou une liste
 *     serait coupé en deux et rendu de travers ;
 *  2. un appel situé DANS un bloc de code (``` … ```) est repoussé APRÈS lui :
 *     un bloc de code interrompu par une puce est illisible, et la clôture
 *     manquante ferait avaler tout le reste de la réponse.
 *
 * Les morceaux de texte vides sont retirés : un appel en tête de réponse ne doit
 * pas produire un `<MarkdownRiche>` vide.
 */
export function segmentsDeReponse(texte: string, outils?: OutilAppel[]): Segment[] {
  const appels = outils || [];
  if (!appels.length) return [{ texte }];

  const avecPos = appels.filter((o): o is OutilAppel & { pos: number } => typeof o.pos === "number");
  const sansPos = appels.filter((o) => typeof o.pos !== "number");
  if (!avecPos.length) return [{ texte }, ...sansPos.map((outil) => ({ outil }))];

  const placements = [...avecPos].sort((a, b) => (a.pos ?? 0) - (b.pos ?? 0));

  /** Ramène une position brute à une frontière sûre (voir les deux précautions). */
  const frontiere = (brut: number, curseur: number): number => {
    const p = Math.max(0, Math.min(brut, texte.length));
    // (0) RIEN DE NOUVEAU À PLACER AVANT : l'appel suit le curseur, et un appel
    //     en position 0 reste donc EN TÊTE (il a eu lieu avant toute réponse).
    //     Sans cette borne, la règle (1 bis) le repousserait après le premier
    //     paragraphe — l'appel se lirait APRÈS du texte qu'il a précédé.
    if (p <= curseur) return curseur;
    // (1) la dernière ligne vide qui PRÉCÈDE la position
    const avant = texte.lastIndexOf("\n\n", Math.max(0, p - 1));
    let borne = avant >= 0 ? avant + 2 : -1;
    // (1 bis) AUCUNE ligne vide avant (réponse d'un seul paragraphe, ou position
    // très tôt) : reculer à 0 placerait l'appel AVANT le texte qu'il a suivi —
    // trompeur. On avance alors jusqu'à la PROCHAINE ligne vide, ou la fin : un
    // appel se lit après ce qui précède, jamais avant.
    if (borne < 0) {
      const apres = texte.indexOf("\n\n", p);
      borne = apres >= 0 ? apres + 2 : texte.length;
    }
    // (2) si ce point tombe dans un bloc de code OUVERT, aller après sa fermeture
    const clotures = (texte.slice(0, borne).match(/```/g) || []).length;
    if (clotures % 2 === 1) {
      const fin = texte.indexOf("```", borne);
      if (fin < 0) return texte.length;            // bloc jamais fermé : on suit la fin
      const saut = texte.indexOf("\n", fin + 3);
      return saut >= 0 ? saut + 1 : texte.length;
    }
    return borne;
  };

  const segments: Segment[] = [];
  let curseur = 0;
  for (const appel of placements) {
    const p = frontiere(appel.pos ?? 0, curseur);
    segments.push({ texte: texte.slice(curseur, p) });
    segments.push({ outil: appel });
    curseur = p;
  }
  segments.push({ texte: texte.slice(curseur) });
  for (const outil of sansPos) segments.push({ outil });
  return segments.filter((s) => !("texte" in s) || s.texte.length > 0);
}
