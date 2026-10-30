# ADR — L'échelle multi-niveaux (2026-09-16)

> **Document IMPORTABLE** : la connaissance durable de la lignée, prête à entrer dans le
> graphe par le processus qui détient `OBRAIN_WRITE=1`.
>
> **Mesuré le 2026-09-16, depuis une session de travail** : l'écriture directe est
> REFUSÉE, et c'est voulu — `manage_adr` répond `écriture impossible (store en lecture
> seule ?)` et `import_docs` ferme la connexion. Un seul processus détient l'écriture
> (c'est l'invariant qui protège le WAL). Le véhicule est donc ce fichier.
>
> Document de reprise détaillé : `docs/ECHELLE-MULTI-NIVEAUX-REPRISE-2026-09-16.md`.

## Décisions

1. **Une seule autorité d'échelle : `palierDe(d, R)`**, dans le module
   **`packages/obrain-dashboard/src/graph/echelle.ts`** (avec `distancePourT`,
   `distancePourPalier`, `viserCran`, `bandeDe`, `milieuBande`), **testé par
   `echelle.test.mjs` — 22 cas** ; `CosmosWeave.tsx` l'IMPORTE.
   Bandes : `BANDE_MONDE 0,34`, `BANDE_DEDANS 0,72`, `PALIER_DEDANS_SIGNAUX 0,25`,
   `PALIER_SIGNAUX 0,08`. Course **logarithmique** : `distancePourT(t,R) = 14R·exp(−t·ln 56)`.
   Le mot affiché, le cran allumé, l'ouverture des étages et le mélange vers les signaux
   s'y réfèrent tous — jamais un second calcul.
2. **La descente est continue, donc le contrôle est un CURSEUR** : `input[type=range]`
   vertical (`writing-mode: vertical-lr`, `z-index: 42`, à gauche), quatre crans de
   Espace (haut) à Signaux (bas), chacun **portant sa matière** lue sur `__etages`.
3. **La loi densifier/approfondir est un module PUR** (`densite.ts`, 18 cas) :
   densifier tant que la matière reçue n'est pas dessinée ; par appel (`offset`) si le
   serveur a coupé ; en élargissant l'échantillon si le niveau est découpé sur le graphe ;
   approfondir seulement quand tout est montré. Invariant :
   `bougeLeNiveau === (geste === "approfondir")`.
4. **Viser et mesurer doivent parler du même monde** : `setFocusQn(cibleN)` — l'ÉTAT, pas
   la référence. Un `ref` qui re-synchronise un état est un **cache**, pas une autorité.
5. **Aucun plafond silencieux, aucune offre muette** : l'offre est bornée à ce que le geste
   sait faire ; le verdict du serveur survit au redessin ; un refus se dit (journal
   `__etages.crans`).
6. **Le moteur arrête ce qui n'a plus de sens** : coupe-circuit de répétition
   (`repetition.rs` : bloc ≥ 40 car. × ≥ 3, ou ligne ≥ 8 car. × ≥ 6, texte ≥ 600 car.), arrêt
   quand le client est parti (drapeau posé par l'écrivain SSE), budget par question
   (`BUDGET_QUESTION_S = 300`). L'arrêt s'écrit au journal, en `WARN`.

## Mesures de référence (instance isolée, chromium CDP)

| ce qui est mesuré | valeur |
|---|---|
| les quatre crans (posés, stabilisés) | `camDist` 1036 → 151,6 → 75,8 |
| verdict de la tenue de cran | `{niv:1,d:62,54}` · `{niv:2,d:12,53}` · `{niv:3,d:4,51,palier:3}` |
| densifier un niveau | 24 → 48 → 72 → 75 groupes dessinés (aucun appel) |
| densifier au-delà du serveur | `offset=200`, puis `offset=400`, puis arrêt après **2 pages vides** (200 lignes d'effectif maximal **1**) |
| chat sans base | `HTTP 200` + SSE → `contexte` → `think` → `texte` (une réponse réelle) |
| coupe-circuit de répétition | réponse réelle de 719 caractères (tableau compris) : **non déclenché** |
| client parti | `client parti — génération arrêtée au tour 1 (23,8 s de modèle cumulées)` |
| conversations structurées | 271 o conservés après rechargement, **2 conversations visibles** (avant correctif : 155 o, 1 — la seconde était écrasée) |
| espace vide (self `achaptal`, 2 nœuds) | aucune erreur, et un bandeau RÉEL : « Cet espace est encore vide — 2 nœuds… [💬 poser une question] [＋ ajouter un monde] » ; espace peuplé → **aucun bandeau** |

## Défauts OUVERTS (mesurés, localisés)

1. **Deux distances pour une seule échelle.** Le palier et la tenue mesurent
   `caméra ↔ cible.mesh.position` ; `t` et `camDist` — donc la barre, le % du curseur et le
   contrôle d'acceptation — mesurent `caméra ↔ ctrl.target`. Le contrôle juge la descente
   sur la mauvaise des deux, d'où un « recul » apparent au cran « signaux ».
   → **unifier sur `cible.mesh.position`**.
2. **`tS` reste à 0** au cran « signaux » alors que le palier 3 est atteint : le nom du cran
   est honoré par la distance, pas par le mélange (le niveau annoncé exige
   `tS > 0,02 && nGraph > 0,5`). Déjà mesuré à 1 ailleurs → la question est « pourquoi pas
   ici ».

## Pièges d'instrument (payés, à ne pas repayer)

`head` sur un contrôle (SIGPIPE tue le script) · `offsetParent` nul pour tout
`position:fixed` (invisible ≠ non affiché) · `input[type=range]` piloté en JS que React
ignore · `tsc` est une **PORTE** · un champ publié non déclaré fait mentir le bundle servi ·
le disque plein (0 octet) tue serveurs et chromium et fait mentir les mesures.
