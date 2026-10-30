# État du Dashboard obrain — synthèse des évolutions (contexte à jour)

## 1 · Ce qu'est devenu ma partie
Du « viewer HTML 3D » (fin de la couche viz P0/P1) → **Dashboard réutilisable**
servi par `obrain viz serve` (binaire RELEASE) :
`packages/obrain-dashboard` (React 18 + TS + Jotai + SCSS + Three.js), package
`@obrain-db/dashboard`, embarquable par URL (`docs/DASHBOARD-EMBED.md`).

Architecture fichiers :
- **état** : `src/app/store.ts` (atomes : stores, centres, échelle, couches,
  overlay, historique, Monde, Organique, suggestions, résultats moteur)
- **chrome canvas** : `App.tsx` (topbar fine, quickbar chips presets sur le
  graphe, dock détail, fil d'Ariane) — plus aucun menu sélecteur obligatoire
- **graphe** : `graph/Graph3D.tsx` — InstancedMesh + segments vertexColors,
  K3/K4/hyper/∂, couleurs communautés, synapses, labels LOD, halos organiques,
  **densité D0-D3 pilotée caméra**, hover voisinage + info-bulle, zoom/pinch,
  click = sélection, double-click = plonger
- **Monde** : `components/StoreOverview.tsx` — galaxie 1 point=1 item par
  coquilles de type réelles, marqueurs modules (stores de code), couches
  Organique/énergie réactives, **fix cognitif : la boucle de rendu tourne
  toujours** (Runtime dev/self affichables)
- **facette** : `components/FacetBar.tsx` — barre bas-centre : Parents/Enfants/
  Fichier/Communauté/∂/Énergie/Strates dépliables inline et ancrables
- **questions** : `components/QueryConsole.tsx` — GQL/Cypher RO, **axes moteur
  mobilisés** (multi-axes), ancrage des résultats sur le graphe, table soignée
- **recherche** : paliers serveur (exact→nom→suffixe→contient) + contexte
  fichier/type + clavier + raccourcis / ⌘K
- **robustesse** : `CanvasBoundary` (plus d'écrans vides muets), skill
  `docs/UX-UI-SKILL.md`, backlog `docs/BACKLOG-OBRAIN-DASHBOARD.md`

## 2 · Serveur obrain (côté viz + données)
- Endpoints : /api/stores, /api/info, /api/viz/overview, /api/viz/modules,
  /api/viz/arcs, /api/viz/subgraph, /api/functions, /api/query (RO).
- **Perf** : binaire release ; **cache de handles RO par store** (overview
  82 ms à chaud) ; cache TTL agrégats modules/arcs ; BFS par planches ;
  sortante par défaut sur gros stores ; SCC sous seuil ; **garde-fous closure**
  (plafond 1500, plus d'ERR_EMPTY).
- **Enrichissement** : ricci provisionné — e1000, drivers-net (44 % nég.) et
  **kernel v6.6 complet (4 779 150 arêtes, 60,6 % négatives)** → couche ∂ active.
- Stores branchés : **Runtime dev (défaut) 342 k nœuds / 1,15 M arêtes**,
  Runtime prod, Self-demo (cognitif réel : Message/ToolCall/Step/PROVES/
  TOUCHES…), Linux complet (1,55 M / 4,78 M), Linux e1000, Obrain rust.
- Service : `scripts/vizserve.sh` (relance rapide ; l'environnement tue les
  processus entre tours → relancer à la main si down).

## 3 · Compréhension de l'ensemble obrain
- Moteur substrate **µs** : agrégats O(1) (0,09 ms), `edges_from` O(degré)
  (store.rs:12931) → le sprint « expansion native » rendra la closure
  quasi instantanée (aujourd'hui 19 s sur kernel en closure profonde).
- Store réel multi-domaines : harness (dev/prod/self) = **nœuds de
  connaissance + projections** (engrams colonnes, énergie, synapses) ; code =
  appel/fichiers/ricci. La vision = **cosmos multi-galaxies** : une galaxie
  par base + arcs de similarité réels ; aletheia (git) & wallet (SQL) à
  intégrer comme galaxies (backlog).

## 4 · Limites connues / à faire
1. Closure profonde µs (expansion native `edges_from`) — chantier prioritaire.
2. Canvas unifié Monde↔Graphe à l'échelle (bloc C).
3. Cosmos multi-galaxies + aletheia/wallet + arcs externes réels.
4. QA visuelle : requiert capture utilisateur (navigateur de l'agent bloqué
   sur IP privée).
