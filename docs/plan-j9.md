# NATIF-J9 — transport vivant du viz : WebSocket streaming (+ deltas, + protobuf)

Base : `feat/transport-ws-j9` @ 62b245b8 (contient J5C+J6+J6B+J7+J8).
Doctrine : « tout instantané ». Vision : un graphe branché a l'inference qui BOUGE.

## Constat (mesure d'entree)
Le viz `/api/viz/subgraph` est deja rapide cote extraction (~25 ms, traversee
indexee) et deja leger sur le fil (gzip negocie J7, 15-31x). Le cout RESTANT,
sur gros subgraph, n'est plus la taille sur le fil ni l'extraction :
- (a) **serialisation serveur** d'un seul gros corps JSON (plusieurs Mo) ;
- (b) **`JSON.parse` client** de ce corps, qui BLOQUE le thread UI d'un coup ;
- (c) **re-fetch complet** a chaque interaction meme si peu a change.

gzip a regle la taille, pas (a)(b)(c). Le POST/GET one-shot est un mur : le
client ne peut RIEN peindre avant d'avoir tout recu, tout parse.

## Ce qui ne change PAS (invariant dur)
Le HTTP JSON+gzip actuel (`/api/viz/subgraph`, `/api/query`) RESTE le defaut et
le repli. Le WS est une voie ADDITIONNELLE, negociee. `/app` sert toujours. Le
rendu 3D (`Graph3D.tsx`, `CosmosWeave.tsx`) ne change pas — seule la SOURCE des
donnees change.

## Phase A (MVP prioritaire) — WebSocket streaming du subgraph
Endpoint `GET /api/viz/subgraph/stream?<memes params que /api/viz/subgraph>`
qui, apres upgrade WS, pousse le subgraph par CHUNKS (lots de noeuds puis
d'aretes), au lieu d'un seul gros corps. Le client peint progressivement.

Trame (frames texte JSON, serveur -> client, non masquees) :
- `{"t":"begin","schema":"obrain/json-graph@1","stats":{node_count,edge_count,truncated},"n_nodes":N,"n_edges":M}`
- `{"t":"nodes","items":[<VizNode>,...]}`  (lots de CHUNK=512)
- `{"t":"edges","items":[<VizEdge>,...]}`  (lots de CHUNK=512)
- `{"t":"end"}`
- `{"t":"error","detail":{...}}` (repli : le client bascule HTTP)

Le serveur EXTRAIT tout (25 ms, indexe — l'extracteur J5-J8 est fige, on n'y
touche pas), puis STREAME la serialisation par lots. Le gain n'est donc pas une
extraction incrementale mais :
- **time-to-first-node** = extraction + serialisation d'UN lot (~ms) au lieu de
  extraction + serialisation du corps ENTIER + gzip ;
- le client parse N petites frames au lieu d'un `JSON.parse` geant bloquant : le
  thread UI respire entre les lots ;
- pipeline : le serveur serialise le lot k+1 pendant que le client parse le k.

## Phase B (si A prouvee) — deltas
Sur un graphe qui vit, pousser seulement les CHANGEMENTS depuis une epoque
cliente connue (comme J8) via le WS, au lieu de re-fetch. Reutilise
`obrain_viz::diff_subgraphs(base, other) -> SubgraphDiff` (DEJA present :
added/removed noeuds+aretes). Repli : re-snapshot complet si le client est trop
en retard (epoque inconnue / trop ancienne).

## Phase C (si A+B prouvees ; sinon differe — CLAUSE ANTI-ENLISEMENT)
Schema `.proto` versionne (Node/Edge/Overlay/Chunk/Delta), negocie. JSON reste
le repli. Attaque le CPU (serialisation + parse) et ~2-3x le fil apres gzip.

## DECISION ARCHI

### Lib WebSocket : handshake RFC6455 + frame writer MAISON (std-only, zero dep)
Le viz serve est explicitement « mini serveur HTTP std-only, zero dependance »,
un thread par connexion (`TcpListener::incoming` -> `thread::spawn` -> `handle`),
IO bloquante, `Connection: close`. C'est le modele IDEAL pour un WS server->client :
on detourne le `TcpStream` apres handshake, on ecrit les frames, on ferme.

`tokio-tungstenite`/`tungstenite` ne sont PAS dans les deps directes de
`obrain-cli` (Cargo.toml : serde, serde_json, flate2). Les tirer imposerait un
runtime **async** tokio dans un serveur **synchrone** thread-par-connexion —
inadequation profonde, gros rebuild, sous pression disque (wami build en //).
La clause anti-enlisement l'interdit.

Or je n'ai besoin, pour la Phase A, que de : (1) le handshake (SHA-1 + base64
du `Sec-WebSocket-Key` + GUID RFC6455), (2) l'ecriture de frames texte NON
masquees serveur->client, (3) une frame close. Les params arrivent dans l'URL du
GET d'upgrade : **aucune lecture de frame client** n'est requise pour A. Total
~120 lignes. SHA-1 (~40 l) et base64 (~15 l) sont implementes inline avec
vecteur de test RFC (`dGhlIHNhbXBsZSBub25jZQ==` -> `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`).
=> **zero nouvelle dependance, zero churn Cargo.toml, esprit std-only preserve.**

### Identite par construction
Le WS et le HTTP partagent le MEME chemin d'extraction. `api_subgraph` est
refactore : `prepare_subgraph(query, config) -> PreparedSubgraph` (Error |
Cached(doc) | Built{styled, cache_key}). HTTP serialise+cache+repond comme avant
(byte-identique). WS canonicalise le meme `styled` (`Subgraph::canonical()`,
idempotent) et streame `canon.nodes`/`canon.edges` DANS l'ordre canonique, avec
`stats = canon.stats()` et le meme schema. Donc l'accumulation client
{schema, stats, nodes, edges} == le corps HTTP (memes ids, memes champs).

### Repli
Client : tente le WS ; sur echec handshake / erreur / `close` avant `end` /
timeout / absence de `WebSocket`, retombe sur `fetch('/api/viz/subgraph')`
(HTTP+gzip) sans erreur visible. Le WS ne remplace jamais le HTTP, il le double.

## PREUVES (banc = COPIE d'un store reel, port 8491-8499 sur 127.0.0.1)
- time-to-first-node : t(1re frame de donnees WS) vs t(corps HTTP complet).
- identite : chunks WS accumules == JSON HTTP (memes ids/champs), script de test.
- repli : WS coupe -> dashboard sur HTTP+gzip, sans erreur.
- non-regression : `/api/viz/subgraph` & `/api/query` inchanges ; `/app` sert ;
  `cargo test --release -p obrain-cli -p obrain-viz` vert ; build dashboard vert.

## TERRITOIRE
Code : `crates/obrain-cli/src/viz_ws.rs` (primitives WS), `.../viz_serve.rs`
(endpoint + refactor), `packages/obrain-dashboard/src/lib/api.ts` (client WS +
repli). NE PAS toucher planner/substrate (J5-J8 figes).
