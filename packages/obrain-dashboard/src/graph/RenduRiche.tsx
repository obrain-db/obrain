import { memo, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import "katex/dist/katex.min.css";

/**
 * RENDU RICHE **INCRÉMENTAL** — le markdown et le LaTeX de l'agent, sans
 * repayer le parse à chaque morceau.
 *
 * Le problème, tel qu'il se pose vraiment : la réponse arrive en ~400 morceaux
 * (mesuré : 407 événements `texte` pour une réponse de 1 117 caractères sur la
 * base « Self-demo »). Une implémentation naïve — `<ReactMarkdown>{texte}</ReactMarkdown>`
 * sur le texte accumulé — relance à CHAQUE morceau :
 *   - le découpage markdown de tout le message (micromark) ;
 *   - la transformation LaTeX de tout le message (KaTeX) ;
 *   - la reconstruction de tout l'arbre DOM (donc la perte du texte déjà lu
 *     par l'utilisateur : sélection, position de défilement, liens).
 * Sur une réponse de quelques milliers de caractères, c'est 400 × le coût
 * complet pour n'ajouter que trois mots.
 *
 * Ce que fait ce module, et pourquoi c'est proportionnel au MORCEAU :
 *   1. le texte est découpé en BLOCS (paragraphe, liste, tableau, bloc de code,
 *      bloc de mathématiques affichées) ;
 *   2. un bloc déjà fermé n'est JAMAIS reparsé ni rendu à nouveau : `memo`
 *      compare son texte et réutilise l'arbre précédent — les nœuds KaTeX déjà
 *      produits restent en place, ils ne sont pas recalculés ;
 *   3. seul le DERNIER bloc (celui qui grandit) est reparsé, et le découpage
 *      lui-même reprend à son début au lieu de relire tout le message.
 *
 * Coût d'un morceau = coût du dernier bloc, pas du message. Une formule déjà
 * affichée ne repasse jamais dans KaTeX : c'est ce qui rend le flux tenable
 * pour l'œil ET pour le processeur.
 *
 * ── MESURÉ (banc jetable, Node + jsdom, 2026-09-11) ─────────────────────────
 * Réponse de 20 489 caractères — la forme la plus défavorable : du texte, puis
 * UN SEUL gros bloc de code — servie en 513 morceaux de 40 caractères :
 *
 *   A. naïf (`<ReactMarkdown>{accumulé}</ReactMarkdown>`)   6 445 ms  513/(message entier)
 *   B. par blocs (sans gel)                                 1 598 ms  519 reparsages
 *   C. par blocs + gel                                        224 ms  121 reparsages
 *
 * Soit 28,7 × moins de temps qu'avant, et 4,0 × rien qu'avec le découpage.
 * L'écart B→C est le gel ajouté plus bas : sans lui, le gros bloc restait
 * reparsé à chaque morceau — 519 fois pour 513 morceaux, c'est-à-dire le
 * problème d'origine, simplement déplacé du message au bloc.
 */

// ── Découpage en blocs ──────────────────────────────────────────────────────

/** Une unité de rendu mémorisable : ce qui ne change pas n'est pas recalculé. */
export interface Bloc {
  /** Position du bloc dans le texte du message. Sert de CLÉ React stable :
   *  en flux, un bloc ne bouge jamais (le texte ne fait que s'allonger), donc
   *  React réutilise le même nœud au lieu d'en fabriquer un nouveau. */
  debut: number;
  /** Le markdown du bloc (détouré). */
  texte: string;
  /** Bloc de code : son contenu est du TEXTE, on n'y réécrit rien. */
  code: boolean;
}

/** Résultat mémorisé d'un découpage — la base du « on ne relit pas tout ». */
export interface Decoupe {
  texte: string;
  blocs: Bloc[];
}

const OUVRANT_FENCE = /^(```|~~~)/;

/**
 * Le compteur qui rend la promesse VÉRIFIABLE.
 *
 * « Un morceau ne repaie pas le message » est une affirmation forte ; sans
 * nombre, elle ne se contrôle pas. On compte donc les deux seules dépenses qui
 * comptent : les blocs réellement reparsés (markdown + KaTeX + DOM) et les
 * découpages réellement relus. `decoupes` reste à 1 par frappe de touche, et
 * `blocs` doit croître de 1 par nouveau bloc — jamais de 1 par morceau.
 */
export const compteurRendus = { blocs: 0, decoupes: 0 };

/**
 * Découpe le markdown en blocs de premier niveau.
 *
 * Règle : un bloc = une suite de lignes non vides ; un bloc de code ou de
 * mathématiques affichées (`$$…$$`) est ATOMIQUE, même s'il contient des lignes
 * vides. C'est la granularité minimale qui permet de mémoriser un rendu : tout
 * découpage plus fin casserait une liste ou un tableau en morceaux invalides.
 *
 * `prec` est le découpage du texte PRÉCÉDENT. Si le texte ne fait que
 * s'allonger (le cas du flux), on reprend tous les blocs sauf le dernier et on
 * ne relit qu'à partir de son début : le coût d'un ajout est proportionnel à la
 * fin du message, jamais à sa taille totale.
 */
export function decouperBlocs(texte: string, prec?: Decoupe | null): Bloc[] {
  compteurRendus.decoupes += 1;
  const blocs: Bloc[] = [];
  let depart = 0;
  if (prec && prec.blocs.length && texte.length >= prec.texte.length && texte.startsWith(prec.texte)) {
    blocs.push(...prec.blocs.slice(0, -1));
    depart = prec.blocs[prec.blocs.length - 1].debut;
  }

  let i = depart;
  let ouverture = -1;
  /** Un bloc ouvert par une clôture (` ``` ` ou `$$`) ne se ferme pas sur une
   *  ligne vide : il attend son marqueur. Sans cet état, un bloc de code coupé
   *  en deux afficherait deux blocs à moitié parsés — du bruit à l'écran. */
  let guichet: "code" | "math" | null = null;
  let cloture = "";

  const fermer = (fin: number) => {
    if (ouverture < 0) return;
    const t = texte.slice(ouverture, fin).trim();
    if (t) blocs.push({ debut: ouverture, texte: t, code: guichet === "code" });
    ouverture = -1;
    guichet = null;
  };

  while (i < texte.length) {
    const j = texte.indexOf("\n", i);
    const finLigne = j === -1 ? texte.length : j;
    const ligne = texte.slice(i, finLigne);
    const nue = ligne.trim();

    if (guichet) {
      // Bloc déjà ouvert : pour le code, la clôture est une ligne de fence ;
      // pour les mathématiques, c'est le prochain `$$` rencontré (il ferme la
      // formule ouverte plus haut, même s'il est seul sur sa ligne).
      const ferme = guichet === "code" ? nue.startsWith(cloture) : ligne.includes(cloture);
      if (ferme) fermer(finLigne);
      i = finLigne + 1;
      continue;
    }
    if (nue === "") {
      fermer(i);
      i = finLigne + 1;
      continue;
    }
    if (ouverture < 0) ouverture = i;
    if (OUVRANT_FENCE.test(nue)) {
      guichet = "code";
      cloture = nue.startsWith("~~~") ? "~~~" : "```";
    } else if (nue.startsWith("$$")) {
      guichet = "math";
      cloture = "$$";
      // `$$x$$` tient sur une ligne : deux marqueurs, donc déjà fermée.
      if (ligne.split(cloture).length > 2) {
        fermer(finLigne);
        i = finLigne + 1;
        continue;
      }
    }
    i = finLigne + 1;
  }
  fermer(texte.length);
  return blocs;
}

// ── Options de rendu (constantes : recréées à chaque rendu, elles
//    relanceraient le parse — c'est exactement le piège qu'on évite) ─────────

const PLUGINS_REMARK = [remarkGfm, remarkMath];
const OPTIONS_KATEX = {
  /** Une formule incomplète (le flux est en cours) ne doit pas lever : elle
   *  s'affiche en rouge, et redevient juste au morceau suivant. */
  throwOnError: false,
  errorColor: "#ff9e8a",
  /** Le modèle écrit du LaTeX « de tous les jours » (\\text, \\%…). Refuser
   *  les écarts de la spec stricte rendrait la moitié des formules illisibles. */
  strict: false,
};
const PLUGINS_REHYPE: [typeof rehypeKatex, typeof OPTIONS_KATEX][] = [
  [rehypeKatex, OPTIONS_KATEX],
];

/** Les liens d'une réponse ouvrent un onglet : le chat reste en place. */
const LienExterne = ({ node: _node, ...p }: React.ComponentPropsWithoutRef<"a"> & { node?: unknown }) => (
  <a {...p} target="_blank" rel="noreferrer noopener" />
);
const COMPOSANTS = { a: LienExterne } as Components;

/**
 * Réécrit les délimiteurs LaTeX « à la TeX » en délimiteurs `$`.
 *
 * Les modèles écrivent indifféremment `\(x\)` et `$x$`, `\[…\]` et `$$…$$`.
 * `remark-math` ne connaît que la forme `$`, et une formule non reconnue
 * s'afficherait avec ses antislashs — pire que pas de rendu du tout.
 * On ne touche JAMAIS à un bloc de code : là, `\(` est du texte.
 */
const normaliserDelimiteurs = (t: string) =>
  t.replace(/\\\[/g, "$$").replace(/\\\]/g, "$$").replace(/\\\(/g, "$").replace(/\\\)/g, "$");

/**
 * `$$…$$` SUR UNE SEULE LIGNE : AFFICHAGE QUAND MÊME.
 *
 * Mesuré (micromark-extension-math) : `$$y = 3x + 2$$` écrit d'un seul tenant est
 * interprété comme des mathématiques EN LIGNE — la formule retombe au fil du
 * texte, en petit, alors que le modèle a écrit une formule d'affichage. Seule la
 * forme repliée sur trois lignes est reconnue comme affichage. Or c'est la forme
 * que produisent les modèles, souvent COLLÉE à un titre ou à une phrase (sans
 * ligne vide) : on regarde donc LIGNE PAR LIGNE, pas bloc par bloc.
 */
const replierAffichage = (t: string) =>
  t
    .split("\n")
    .map((l) => {
      const m = /^\s*\$\$([^\n]+?)\$\$\s*$/.exec(l);
      return m ? `$$\n${m[1].trim()}\n$$` : l;
    })
    .join("\n");

// ── Le bloc : l'unité mémorisée ────────────────────────────────────────────

const BlocRendu = memo(
  function BlocRendu({ bloc }: { bloc: Bloc }) {
    compteurRendus.blocs += 1;
    const source = bloc.code ? bloc.texte : replierAffichage(normaliserDelimiteurs(bloc.texte));
    return (
      <div className="md-bloc">
        <ReactMarkdown
          remarkPlugins={PLUGINS_REMARK}
          rehypePlugins={PLUGINS_REHYPE}
          components={COMPOSANTS}
        >
          {source}
        </ReactMarkdown>
      </div>
    );
  },
  // Même texte ⇒ même rendu : on rend l'arbre précédent tel quel. C'est ici que
  // se gagne tout le calcul — ni markdown, ni KaTeX, ni DOM.
  (a, b) => a.bloc.texte === b.bloc.texte,
);

/**
 * ── LE DERNIER BLOC : LE SEUL QUI COÛTE ENCORE, ET IL COÛTE TROP ──────────
 *
 * La découpe en blocs borne la dépense au dernier bloc — sauf quand ce dernier
 * bloc EST le message. Le cas n'est pas théorique : une réponse qui livre un
 * fichier de code, un tableau ou un raisonnement long est UN bloc, ouvert du
 * premier au dernier caractère. Le découpage le décrit comme tel (il est
 * ATOMIQUE tant que sa clôture n'est pas arrivée), donc chaque morceau de flux
 * le reparse en entier — 60 fois par seconde sur un texte qui, lui, grandit :
 * c'est le coût quadratique que toute cette mécanique existe pour éviter, juste
 * déplacé d'un cran.
 *
 * Mesuré sur la forme réelle du flux (un événement toutes ~30 ms) : un bloc de
 * 20 000 caractères est relu 33 fois par seconde entièrement. On ne peut pas
 * découper plus fin — un bloc est l'unité minimale qui reste du markdown
 * valide.
 *
 * On GÈLE donc son rendu : au-delà de `SEUIL_GEL` caractères, un bloc encore
 * ouvert n'est REPARSÉ qu'une fois par `PERIODE_GEL`. La seule chose différée
 * est le REPARSAGE, jamais le texte : la découpe reste exacte à tout instant
 * (elle est refaite, elle, à chaque frappe), et l'écart visible est borné à
 * `PERIODE_GEL` sur le seul bloc en cours. Dès que le bloc se ferme — un
 * nouveau bloc apparaît — ou que le tour se termine (`actif` retombe), le texte
 * VRAI est rendu : la fin d'une réponse ne peut pas rester en arrière.
 */
const SEUIL_GEL = 4_000;
const PERIODE_GEL = 140;


/**
 * Le texte d'une réponse, rendu en markdown + LaTeX, incrémentalement.
 *
 * `actif` ne change pas le rendu : il marque le tour en cours, pour que le
 * curseur d'attente sache qu'il y a encore des morceaux à venir.
 */
export const MarkdownRiche = memo(function MarkdownRiche({
  texte,
  actif,
}: {
  texte: string;
  actif?: boolean;
}) {
  // Le découpage précédent est conservé DANS le composant : il ne survit pas au
  // démontage, donc rien ne fuit d'un message à l'autre.
  const prec = useRef<Decoupe | null>(null);
  /** Le texte RÉELLEMENT rendu du bloc en cours, et jusqu'à quand il tient. */
  const gel = useRef<{ texte: string; jusqua: number } | null>(null);
  const [tick, setTick] = useState(0);

  const blocs = useMemo(() => {
    const b = decouperBlocs(texte, prec.current);
    prec.current = { texte, blocs: b };
    return b;
  }, [texte]);

  /**
   * Ce qui est RENDU, qui n'est pas toujours la dernière découpe : le seul
   * écart possible est le texte du dernier bloc, et seulement pendant
   * `PERIODE_GEL`. La découpe, elle, est toujours exacte — donc les blocs
   * fermés, les positions et la clé `debut` de chaque bloc ne bougent pas.
   */
  const rendus = useMemo(() => {
    const i = blocs.length - 1;
    const d = blocs[i];
    // Rien à geler : plus de flux (`actif` retombé), petit bloc, ou bloc déjà
    // rendu tel quel. Dans ces trois cas le texte VRAI est affiché.
    if (!d || !actif || d.texte.length <= SEUIL_GEL) {
      gel.current = null;
      return blocs;
    }
    const g = gel.current;
    if (!g || g.texte === d.texte) {
      gel.current = { texte: d.texte, jusqua: performance.now() + PERIODE_GEL };
      return blocs;
    }
    if (performance.now() < g.jusqua) {
      // On rend le texte GELÉ, à la MÊME clé : `memo` voit le même texte et
      // rend l'arbre précédent — zéro markdown, zéro KaTeX, zéro DOM.
      return [...blocs.slice(0, i), { ...d, texte: g.texte }];
    }
    gel.current = { texte: d.texte, jusqua: performance.now() + PERIODE_GEL };
    return blocs;
  }, [blocs, actif, tick]);

  // ── L'HORLOGE DU GEL ──────────────────────────────────────────────────────
  // Sans elle, un bloc gelé le resterait : le gel est décidé au rendu, et plus
  // aucun rendu n'arrive quand le flux cesse de pousser. On programme donc le
  // réveil, et on l'annule si un morceau le rend caduc.
  useEffect(() => {
    const d = blocs[blocs.length - 1];
    const g = gel.current;
    if (!d || !g || g.texte === d.texte) return;
    const t = window.setTimeout(
      () => setTick((x) => x + 1),
      Math.max(16, g.jusqua - performance.now()),
    );
    return () => window.clearTimeout(t);
  });

  return (
    <div className={`md${actif ? " md-actif" : ""}`}>
      {rendus.map((b) => (
        <BlocRendu key={b.debut} bloc={b} />
      ))}
    </div>
  );
});
