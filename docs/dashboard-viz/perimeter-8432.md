# Périmètre :8432 — Atlas & relief (2026-09-10)

Inventaire **issu du code** (`crates/obrain-cli/src/viz_serve.rs`) et **mesuré à chaud**
sur le serveur live. Contrat : après warm, **tout répond ≤ ~1 ms** ; le froid est
payé une fois par agrégat (durable disque+mémoire, validé par mtime du store).

## Endpoints API

| Route | Rôle | Cache | Chaud (mesuré) | Client |
|---|---|---|---|---|
| `/` · `/app/…` | SPA dashboard | statique disque | — | navigateur |
| `/api/stores` | presets | — | 0,41 ms | dashboard |
| `/api/info` | estimation store | — | 0,34 ms | Graph3D |
| `/api/functions` | recherche symboles (4 paliers) | durable `fn` | ≤1 ms | autocomplete/ancres |
| `/api/query` | GQL/Cypher lecture | durable `q` | 0,65 ms | console/projection |
| `/api/viz/overview` | nœuds/arêtes/histo | durable `ov` | 0,46–0,58 ms | Monde |
| `/api/viz/modules` | racines + rank | durable `mod` | 0,55 ms | Monde |
| `/api/viz/arcs` | arcs CALLS inter-modules | durable `arcs` | 0,63–0,67 ms | tissu |
| `/api/viz/space` | sous-espace (ensembles ± arcs) | durable `sp` par (store,prefix,rank,links) | 0,52–0,69 ms | anneaux/hover/branches |
| `/api/viz/subgraph` | sous-graphe unitaire | RO handle + résolution | 0,70 ms | Graph3D |
| `/api/viz/status` | monitoring (caches/uptime/stores) | — | 0,66 ms | ops |
| `/sitemap.xml` | pages générées (stores + projets) | disque `mod` | 1,17 ms | crawlers |

Mesures : runtime-dev **et** linux-full3 (1,5 M nœuds) — toutes 200, ≤1,2 ms.

## Moteurs de rapidité en place
1. **Caches durables disque+mémoire** par agrégat (mtime du store = snapshot immuable) ; survie aux redémarrages ; répertoire `OBRAIN_VIZ_CACHE_DIR` (défaut `/tmp/obrain-viz-cache`).
2. **Une seule agrégation par sous-espace** (clés normalisées), `links=0` pour hover/rameaux.
3. **Préchauffage automatique** des enfants + petits-enfants (cascade) après chaque niveau ; **warm au déploiement** (`scripts/vizserve.sh`).
4. **Requêtes lecture et recherche** cachées durablement (console, projection, autocomplete).
5. Monitoring : `GET /api/viz/status` (fichiers par type).

## Contrat « MCP viable ~1 ms »
Tout **outil/endpoint MCP** qui repose sur un agrégat du tableau ci-dessus DOIT passer
par la même voie cache (ou par le endpoint HTTP chaud) — jamais rescaner :
- si le bridge MCP appelle le serveur HTTP : il hérite des caches (garanti ≤ ~1 ms à chaud) ;
- si l'outil MCP ouvre directement le store (`ObrainDB`), il doit réutiliser le module
  de cache d'agrégats (à unifier côté moteur — cf. Étape C-full) ou s'en remettre au
  endpoint HTTP local pour les appels fréquents.
Vérification rejouable : `bash scripts/smoke-backend.sh` (tous < 50 ms, mesurés < 1 ms).

## Tissage base
- Décisions & backlog : `docs/dashboard-viz/GRAPH.md`.
- Codes : `viz_serve.rs` (HTTP + caches) · `mcp_server.rs` (outils) · `vizserve.sh` (warm).
- Ce document est le **relief** : à mettre à jour à chaque nouvel endpoint.

## Relief code (topologie dashboard, extrait du graphe moteur + disque)
| Fichier | Lignes (disque) | Rôle |
|---|---|---|
| components/StoreOverview.tsx | ~1250 | Monde (scène/interactions — cible fin Étape F) |
| graph/Graph3D.tsx | ~410 | graphe unitaire |
| components/Inspector/Projection/CommandBar… | 60-170 | chrome/requêtes |
| graph/monde/util.ts | — | primitives pures extraites (Étape F-1) |

→ Découpage restant (F-2) : extraire scène (draw/loop) et interactions (hover/clavier/historique)
en hooks dédiés (`useMondeScene`, `useMondeNav`) — tracé dans GRAPH.md.
