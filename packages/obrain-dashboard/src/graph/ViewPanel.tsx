import { useState, type ReactNode } from "react";
import { PLAN_CSS, PLAN_ORDER } from "./grammar";

/**
 * CHROME COMMUN DES VUES — panneau, fil d'étage, légende.
 *
 * Les vues partageaient déjà la grammaire de touches et le relief (voir
 * `grammar.ts`). Il restait la façon de SE LIRE : chaque vue écrivait son
 * propre panneau, son propre titre, son propre fil d'Ariane, sa propre
 * légende — d'où des hiérarchies et des libellés qui divergeaient d'une vue à
 * l'autre alors qu'elles montrent le même univers à des échelles différentes.
 *
 * Ces trois briques figent ce qui doit être identique :
 *   - `ViewPanel`      : le contenant (titre, sous-titre, position) ;
 *   - `LevelCartouche` : où l'on est (« Atlas › base › branche ») ;
 *   - `PlanLegend`     : les trois plans, mêmes couleurs, même vocabulaire.
 * Le contenu propre à chaque vue reste libre à l'intérieur.
 */

/**
 * Contenant de panneau commun (mêmes classes, même position, même hiérarchie).
 *
 * REPLIABLE PARTOUT, et replié d'office sur mobile : un panneau de 252 px sur
 * un écran de 390 px mangeait la vue et ne pouvait pas être rangé — le canvas
 * passait derrière. Le repli est dans le composant partagé, donc identique dans
 * toutes les vues, et l'état est mémorisé par panneau : replier une fois ne se
 * rejoue pas à chaque bascule de vue.
 */
export function ViewPanel({
  title, subtitle, side = "left", width = 340, children, style, collapsible = true, id,
}: {
  title: string;
  subtitle?: string;
  side?: "left" | "right";
  width?: number;
  children?: ReactNode;
  style?: React.CSSProperties;
  collapsible?: boolean;
  /** Clé de mémorisation du repli (par défaut : le titre). */
  id?: string;
}) {
  // La clé distingue MOBILE et BUREAU : sans cela, replier un panneau sur un
  // téléphone le laissait replié sur grand écran (constaté — un passage mobile
  // avait rendu les panneaux du bureau invisibles, et fait échouer le contrôle
  // de continuité). Le choix de l'utilisateur est mémorisé PAR contexte.
  const cle = (mode: string) => `obrain.panel.${id || title}.${mode}`;
  const mode = () => (typeof window === "undefined" || window.innerWidth > 720 ? "desktop" : "mobile");
  const [open, setOpen] = useState<boolean>(() => {
    try {
      const saved = localStorage.getItem(cle(mode()));
      if (saved !== null) return saved === "1";
    } catch { /* navigation privée : on retombe sur le défaut */ }
    // Défaut : replié sur mobile (l'écran est petit, le canvas d'abord), ouvert
    // ailleurs (comportement historique du bureau).
    return mode() === "desktop";
  });
  const basculer = () => {
    setOpen((o) => {
      try { localStorage.setItem(cle(mode()), o ? "0" : "1"); } catch { /* ignoré */ }
      return !o;
    });
  };
  const visibles = collapsible ? (open ? children : null) : children;

  return (
    <div
      className={"ov-panel" + (collapsible && !open ? " replie" : "")}
      style={{
        left: side === "left" ? 12 : "auto",
        right: side === "right" ? 12 : "auto",
        maxWidth: width,
        ...style,
      }}
    >
      <div className="ov-panel-tete">
        <h2 style={{ margin: 0, fontSize: 13 }}>{title}</h2>
        {collapsible && (
          <button className="ov-panel-repli" onClick={basculer}
            aria-expanded={open}
            aria-label={open ? `Replier « ${title} »` : `Déplier « ${title} »`}
            title={open ? "Replier ce panneau" : "Déplier ce panneau"}>
            {open ? "▾" : "▸"}
          </button>
        )}
      </div>
      {subtitle && open && (
        <div className="q-intro" style={{ marginTop: 4, opacity: 0.85 }}>{subtitle}</div>
      )}
      {visibles}
    </div>
  );
}

/**
 * Fil d'étage : où l'on se trouve dans la descente, et comment remonter. Chaque
 * niveau franchi se clique pour revenir — le même geste dans toutes les vues.
 *
 * `title` : la RAISON d'être de l'étape, quand son libellé ne suffit pas. Un
 * appelant qui la fournit attend qu'elle soit AFFICHÉE — la laisser hors du type
 * l'aurait fait disparaître en silence (mesuré : TypeScript n'applique pas le
 * contrôle des propriétés excédentaires à un objet construit dans un `.map()`,
 * donc une infobulle non déclarée compile sans un mot et ne s'affiche jamais).
 */
export function LevelCartouche({
  levels,
}: {
  levels: { label: string; icon?: string; onClick?: () => void; current?: boolean; title?: string }[];
}) {
  return (
    <div className="chips" style={{ margin: "6px 0", flexWrap: "wrap" }}>
      {levels.map((l, i) => (
        <span key={i} style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
          {i > 0 && <span style={{ opacity: 0.45, fontSize: 11 }}>›</span>}
          <button
            className={l.current ? "chip on" : "chip"}
            onClick={l.onClick}
            disabled={!l.onClick}
            title={l.title ?? (l.onClick ? `Revenir à « ${l.label} »` : l.label)}
          >
            {l.icon ? `${l.icon} ` : ""}{l.label}
          </button>
        </span>
      ))}
    </div>
  );
}

/**
 * Légende des trois plans — MÊME vocabulaire et MÊMES couleurs que la palette
 * partagée. La vue fournit la phrase qui explique ce qu'elle montre ; elle ne
 * redéfinit jamais les couleurs ni les noms des plans.
 */
export function PlanLegend({ caption }: { caption?: ReactNode }) {
  return (
    <div className="q-intro" style={{ marginTop: 6 }}>
      {caption && <>{caption} </>}
      <b>Trois plans</b> :{" "}
      {PLAN_ORDER.map((p, i) => (
        <span key={p}>
          {i > 0 ? " · " : ""}
          <span style={{ color: PLAN_CSS[p] }}>{p}</span>
        </span>
      ))}
      .
    </div>
  );
}
