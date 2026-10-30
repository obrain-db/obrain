/**
 * ── STRUCTURER DES CONVERSATIONS ─────────────────────────────────────────────
 *
 * Module PUR (aucun import de React, aucun accès direct au stockage) : la partie
 * qui DÉCIDE — titre, rangement, tri, fusion, migration — se teste sans
 * navigateur, et `conversations.test.mjs` le fait.
 *
 * ── CE QUI EXISTAIT, ET CE QUI MANQUAIT ─────────────────────────────────────
 *
 * L'historique du chat était UNE liste, sous UNE clé par store
 * (`obrain.chat.v1.<store>`) : une seule conversation par base, sans nom, sans
 * rangement, sans reprise possible d'une autre. Trois conséquences mesurables :
 *   · on ne pouvait pas garder deux échanges séparés (le second écrasait le
 *     contexte du premier) ;
 *   · rien ne nommait une conversation — on reprenait « la » conversation, sans
 *     savoir de quoi elle parlait ;
 *   · il n'existait aucun rangement : ni dossier, ni épingle, ni ordre choisi.
 *
 * ── LA LOI ──────────────────────────────────────────────────────────────────
 *
 *  · Une conversation a un ID stable, un TITRE (tiré de sa première question,
 *    renommable), une date de dernière activité (`maj`), un DOSSIER facultatif
 *    (une étiquette libre : « travail », « idées »…), et un ÉPINGLE.
 *  · Le TRI est une DÉCISION, pas un effet de bord : épinglées d'abord, puis
 *    `maj` décroissante. Deux conversations de même date ne peuvent pas danser
 *    d'un rendu à l'autre — l'ordre est total (départage par ID).
 *  · MIGRATION : la clé v1 est LUE, jamais écrasée en silence. Un historique
 *    existant devient UNE conversation titrée par sa première question, et la
 *    migration est idempotente (une seconde lecture ne redouble rien).
 *  · On ne supprime jamais la DERNIÈRE conversation : un état vide serait un
 *    cul-de-sac, et l'écran doit toujours pouvoir recevoir une question.
 */

/** Un message du fil — forme minimale, compatible avec celle de la vue. */
export interface TourMin {
  role: "user" | "assistant";
  texte: string;
  [k: string]: unknown;
}

export interface Conversation<T extends TourMin = TourMin> {
  /** Identifiant stable (temps + suffixe), jamais réutilisé. */
  id: string;
  /** Nom affiché. Vide = « sans titre » (on ne fabrique pas un faux nom). */
  titre: string;
  /** Première création, en ms. */
  cree: number;
  /** Dernière activité, en ms — c'est elle qui ordonne. */
  maj: number;
  /** Étiquette de rangement libre ("" = non rangée). */
  dossier: string;
  /** Épinglée : elle reste en tête quel que soit son âge. */
  epingle: boolean;
  tours: T[];
}

export interface Etat<T extends TourMin = TourMin> {
  courante: string;
  liste: Conversation<T>[];
}

/** Clé de rangement par store : deux bases ne se mélangent jamais. */
export const cleEtat = (store: string) => `obrain.chat.v2.${store}`;
/** Clé de l'ancien format (une seule conversation) — LUE pour migrer. */
export const cleAncienne = (store: string) => `obrain.chat.v1.${store}`;

/** Identifiant court, unique dans un même milliseconde. */
export function nouvelId(t: number = Date.now(), suffixe: string = ""): string {
  const s = suffixe || Math.random().toString(36).slice(2, 7);
  return `${t.toString(36)}-${s}`;
}

/**
 * Le titre d'une conversation, tiré de sa PREMIÈRE question.
 *
 * On ne résume pas, on ne devine pas : on cite. Un titre inventé ferait croire à
 * un classement que personne n'a fait — le même défaut que la couleur décorative
 * corrigée ailleurs dans cette vue. Tronqué à `max` caractères, sur un mot si
 * possible, pour que la liste reste lisible.
 */
export function titreAuto(tours: TourMin[], max = 42): string {
  const q = tours.find((t) => t.role === "user" && String(t.texte || "").trim());
  if (!q) return "";
  const plat = String(q.texte).replace(/\s+/g, " ").trim();
  if (plat.length <= max) return plat;
  const coupe = plat.slice(0, max);
  const espace = coupe.lastIndexOf(" ");
  return (espace > max * 0.6 ? coupe.slice(0, espace) : coupe).trim() + "…";
}

/** Vrai pour un titre qui n'apprend rien (vide, ou « sans titre »). */
export const titreVide = (t?: string) => !t || !t.trim() || /^sans titre$/i.test(t.trim());

/** Une nouvelle conversation, vide. */
export function nouvelleConversation<T extends TourMin = TourMin>(t: number = Date.now(), id?: string): Conversation<T> {
  return { id: id ?? nouvelId(t), titre: "", cree: t, maj: t, dossier: "", epingle: false, tours: [] };
}

/** L'état d'un compte neuf : une conversation vide, prête à recevoir. */
export function etatNeuf<T extends TourMin = TourMin>(t: number = Date.now()): Etat<T> {
  const c = nouvelleConversation<T>(t);
  return { courante: c.id, liste: [c] };
}

/** La conversation courante, ou la première — jamais `undefined` si la liste vit. */
export function courante<T extends TourMin>(e: Etat<T>): Conversation<T> | undefined {
  return e.liste.find((c) => c.id === e.courante) ?? e.liste[0];
}

/**
 * LE TRI — la seule autorité d'ordre.
 *
 * Épinglées d'abord (les plus récentes entre elles), puis les autres par `maj`
 * décroissante. À date égale, l'ID départage : sans ce départage, l'ordre
 * dépendrait de l'ordre du tableau, et deux rendus pourraient différer.
 */
export function trier<T extends TourMin>(liste: Conversation<T>[]): Conversation<T>[] {
  return [...liste].sort((a, b) => {
    if (a.epingle !== b.epingle) return a.epingle ? -1 : 1;
    if (a.maj !== b.maj) return b.maj - a.maj;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}

/** Les dossiers présents, triés — plus les conversations non rangées comptées. */
export function dossiers<T extends TourMin>(e: Etat<T>): { nom: string; n: number }[] {
  const par = new Map<string, number>();
  for (const c of e.liste) {
    const k = c.dossier.trim();
    par.set(k, (par.get(k) ?? 0) + 1);
  }
  return [...par.entries()]
    .map(([nom, n]) => ({ nom, n }))
    .sort((a, b) => (a.nom === "" ? 1 : b.nom === "" ? -1 : a.nom.localeCompare(b.nom)));
}

/** Écrit les tours de la conversation COURANTE (le geste de la vue). */
export function majCourante<T extends TourMin>(e: Etat<T>, tours: T[], t: number = Date.now()): Etat<T> {
  const c = courante(e);
  const titre = c && titreVide(c.titre) ? titreAuto(tours) : (c?.titre ?? "");
  return {
    ...e,
    liste: e.liste.map((x) => (c && x.id === c.id ? { ...x, tours, maj: t, titre } : x)),
  };
}

export function ajouter<T extends TourMin>(e: Etat<T>, t: number = Date.now()): Etat<T> {
  const c = nouvelleConversation<T>(t);
  return { courante: c.id, liste: [c, ...e.liste] };
}

export function choisir<T extends TourMin>(e: Etat<T>, id: string): Etat<T> {
  return e.liste.some((c) => c.id === id) ? { ...e, courante: id } : e;
}

export function renommer<T extends TourMin>(e: Etat<T>, id: string, titre: string): Etat<T> {
  return { ...e, liste: e.liste.map((c) => (c.id === id ? { ...c, titre: titre.trim() } : c)) };
}

export function ranger<T extends TourMin>(e: Etat<T>, id: string, dossier: string): Etat<T> {
  return { ...e, liste: e.liste.map((c) => (c.id === id ? { ...c, dossier: dossier.trim() } : c)) };
}

export function epingler<T extends TourMin>(e: Etat<T>, id: string): Etat<T> {
  return { ...e, liste: e.liste.map((c) => (c.id === id ? { ...c, epingle: !c.epingle } : c)) };
}

/**
 * Supprimer — et ne JAMAIS laisser la liste vide.
 *
 * Un état sans aucune conversation obligerait l'écran à en fabriquer une à la
 * volée (donc à mentir sur la date), ou à refuser la saisie. On supprime celle
 * qu'on demande, et s'il n'en reste aucune, on en ouvre une neuve.
 */
export function supprimer<T extends TourMin>(e: Etat<T>, id: string, t: number = Date.now()): Etat<T> {
  const reste = e.liste.filter((c) => c.id !== id);
  if (!reste.length) return etatNeuf<T>(t);
  return { courante: e.courante === id ? (trier(reste)[0]?.id ?? reste[0].id) : e.courante, liste: reste };
}

/**
 * MIGRATION de l'ancien format (une liste de tours sous `obrain.chat.v1.<store>`).
 *
 * Elle est IDEMPOTENTE et NON DESTRUCTIVE : on ne réécrit pas l'ancienne clé, on
 * en fait une conversation titrée par sa première question. Une liste vide ne
 * fabrique rien (il n'y a rien à migrer), et un état v2 déjà présent n'est jamais
 * écrasé par la migration.
 */
export function migrer<T extends TourMin>(ancien: T[] | null | undefined, t: number = Date.now()): Etat<T> | null {
  if (!Array.isArray(ancien) || !ancien.length) return null;
  const c = nouvelleConversation<T>(t);
  const propre = ancien.map((x) => ({ ...x, encours: false, reprise: false, erreurReprenable: false }));
  return { courante: c.id, liste: [{ ...c, tours: propre, titre: titreAuto(propre) }] };
}

/** Ce qui est réellement écrit : borné, et sans tour « en cours ». */
export function aPersister<T extends TourMin>(e: Etat<T>, maxTours = 40): Etat<T> {
  return {
    courante: e.courante,
    liste: e.liste.map((c) => ({
      ...c,
      tours: c.tours.filter((t_) => !(t_ as { encours?: boolean }).encours).slice(-maxTours),
    })),
  };
}
