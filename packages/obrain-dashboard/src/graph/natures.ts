/**
 * ── LA NATURE DE CHAQUE TYPE : UNE SEULE DÉCLARATION, PAR IDENTITÉ ──────────
 *
 * CE QUI MANQUAIT, MESURÉ LE 2026-09-14 (service 8432, 7 bases servies) :
 *
 *  1. **27 types distincts** existent réellement (union des `by_label` de
 *     `/api/viz/overview`, base par base). Attention : `/api/viz/signatures`
 *     n'en montre que **10 par base** — il est tronqué, et une table bâtie sur
 *     lui serait fausse dès la deuxième base.
 *
 *  2. **Aucune vue ne colore par identité.** `grammar.ts` exporte
 *     `typeColor(i) = TYPE_PALETTE[i % 10]`, nommé « couleur d'un TYPE » et
 *     documenté « même index ⇒ même couleur partout » — mais ses **6 appels**
 *     passent tous un ordinal qui veut dire autre chose : un bucket de temps
 *     (`CosmosWeave.tsx:2221`), un bucket de distribution (`:1166`), la
 *     profondeur (`StageContinu.tsx:601`), un `community_id` (`:2412`). Le nom
 *     et le commentaire promettaient une garantie que le code ne tenait pas.
 *
 *  3. **La palette elle-même ne distingue pas 10 types** : mesuré, sa distance
 *     minimale est de **15** entre `#ffd166` (ToolResult) et `#ffe066` (Method) —
 *     deux jaunes qu'aucun œil ne sépare. À 27 types, la couleur seule ne peut
 *     donc pas porter l'identité : elle porte une identité STABLE (c'est ce que
 *     cette table garantit), et l'échelle garde le reste (taille, libellé).
 *
 *  4. **La partition qui compte était écrite deux fois** : `COG = ["Message",
 *     "ToolCall", "Session", "Step", "SelfState", "ToolResult"]` en
 *     `StageContinu.tsx:29`, la MÊME liste recopiée en clair à `:1087`, et une
 *     troisième liste de types à `StoreOverview.tsx:180`. C'est exactement le
 *     défaut que l'en-tête de `fratrie.ts` décrit pour la parenté.
 *
 * LA RÈGLE, ET ELLE EST UNIQUE : un type est déclaré ICI, une fois, par son
 * nom ; tout ce qui a besoin de savoir ce qu'un type EST (sa famille, sa
 * couleur) le demande à `natureDe()` et ne devine plus. Un type inconnu n'est ni
 * rangé dans « cognition » ni dans « code » : il est déclaré INCONNU et peint
 * NEUTRE — la doctrine déjà écrite en `CosmosWeave.tsx:2401` (« la couleur doit
 * dire quelque chose — ou rien »).
 *
 * CE QUE CETTE TABLE NE PRÉTEND PAS :
 *  · les TROIS PLANS (`connaissance` / `self` / `utilisateur`) restent
 *    l'autorité du SERVEUR qui les publie par monde (`nature` dans l'API) ; il
 *    n'y a pas ici de seconde taxinomie des plans ;
 *  · `famille` est une partition binaire — cognition (ce que l'agent a fait) ou
 *    code (la structure d'un dépôt) — parce que c'est celle que le code
 *    utilisait déjà ; elle n'invente pas de troisième sens ;
 *  · elle ne force PERSONNE à la lire. Les vues qui gardent une liste en dur
 *    sont comptées par `scripts/natures-check.mjs` : la dette est visible, elle
 *    ne peut que diminuer.
 *
 * ── CONTRAT DE FORME (lu par `scripts/natures-check.mjs`) ───────────────────
 * Une ligne par type, dans cet ordre de champs, et rien d'autre :
 *   Nom: { famille: "cognition" | "code", couleur: "#rrggbb", libelle: "…" },
 * La garde ÉCHOUE si le nombre de lignes lues diffère du nombre de couleurs
 * déclarées : une dérive de format doit se voir, pas passer en silence.
 */

/** Cognition = ce que l'agent a produit. Code = la structure d'un dépôt. */
export type Famille = "cognition" | "code" | "inconnue";

export interface Nature {
  famille: Famille;
  /** Couleur STABLE, propre à ce type et à aucun autre (vérifié par la garde). */
  couleur: string;
  /** Nom lisible, pour une légende : jamais un identifiant technique. */
  libelle: string;
}

/**
 * ── UN TYPE INCONNU RESTE NEUTRE ────────────────────────────────────────────
 *
 * Un type publié par le serveur et non déclaré ici ne doit ressembler à RIEN de
 * connu : sinon une donnée neuve se lit comme une donnée ancienne, et la
 * confusion est silencieuse. Le gris est celui que la scène utilise déjà pour
 * « aucun sens à peindre » (`CosmosWeave.tsx:2412`).
 */
export const NATURE_INCONNUE: Nature = {
  famille: "inconnue",
  couleur: "#8a93a6",
  libelle: "type non déclaré",
};

/**
 * Les 27 types réellement présents, mesurés le 2026-09-14.
 *
 * L'ordre des couleurs suit le cercle chromatique et n'a AUCUN sens
 * sémantique : la couleur distingue, elle n'explique pas. Ce qui explique, c'est
 * `famille` — et ce que l'échelle ajoute : la taille dit l'effectif, le libellé
 * dit le nom.
 */
export const NATURES: Record<string, Nature> = {
  // ── cognition : l'activité de l'agent (les 6 premières étaient déjà la liste
  //    `COG` de StageContinu, recopiée deux fois ; elles vivent ici désormais) ──
  Message: { famille: "cognition", couleur: "#eb46a9", libelle: "message" },
  ToolCall: { famille: "cognition", couleur: "#bd557c", libelle: "appel d'outil" },
  ToolResult: { famille: "cognition", couleur: "#eb4660", libelle: "résultat d'outil" },
  Step: { famille: "cognition", couleur: "#bd5c55", libelle: "étape" },
  SelfState: { famille: "cognition", couleur: "#eb7646", libelle: "état de soi" },
  Turn: { famille: "cognition", couleur: "#bd8a55", libelle: "tour" },
  Session: { famille: "cognition", couleur: "#ebbf46", libelle: "session" },
  Procedure: { famille: "cognition", couleur: "#bdb855", libelle: "procédure" },
  Note: { famille: "cognition", couleur: "#cdeb46", libelle: "note" },
  Goal: { famille: "cognition", couleur: "#93bd55", libelle: "objectif" },
  Self: { famille: "cognition", couleur: "#84eb46", libelle: "soi" },
  Skill: { famille: "cognition", couleur: "#65bd55", libelle: "compétence" },
  // ── code : la structure d'un dépôt ──
  Function: { famille: "code", couleur: "#46eb51", libelle: "fonction" },
  Variable: { famille: "code", couleur: "#55bd73", libelle: "variable" },
  Method: { famille: "code", couleur: "#46eb9a", libelle: "méthode" },
  File: { famille: "code", couleur: "#55bda1", libelle: "fichier" },
  Class: { famille: "code", couleur: "#46ebe3", libelle: "classe" },
  Field: { famille: "code", couleur: "#55aabd", libelle: "champ" },
  Interface: { famille: "code", couleur: "#46a9eb", libelle: "interface" },
  Type: { famille: "code", couleur: "#557cbd", libelle: "type" },
  Struct: { famille: "code", couleur: "#4660eb", libelle: "structure" },
  Module: { famille: "code", couleur: "#5c55bd", libelle: "module" },
  Enum: { famille: "code", couleur: "#7646eb", libelle: "énumération" },
  Table: { famille: "code", couleur: "#8a55bd", libelle: "table" },
  Project: { famille: "code", couleur: "#bf46eb", libelle: "projet" },
  Macro: { famille: "code", couleur: "#b855bd", libelle: "macro" },
  Branch: { famille: "code", couleur: "#eb46cd", libelle: "branche" },
};

/** Tous les types déclarés — l'entrée de la garde. */
export const TYPES_DECLARES: string[] = Object.keys(NATURES);

/** La nature d'un type. Un type inconnu rend la nature INCONNUE, jamais un défaut. */
export const natureDe = (type?: string | null): Nature =>
  (type && NATURES[type]) || NATURE_INCONNUE;

/** Vrai pour « ce que l'agent a produit » — remplace les listes en dur des vues. */
export const estCognition = (type?: string | null): boolean =>
  natureDe(type).famille === "cognition";

/** Vrai pour « la structure d'un dépôt ». */
export const estCode = (type?: string | null): boolean =>
  natureDe(type).famille === "code";

/** La couleur stable d'un type — la seule voie par laquelle une identité se peint. */
export const couleurDuType = (type?: string | null): string => natureDe(type).couleur;
