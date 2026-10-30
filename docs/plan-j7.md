# NATIF-J7 — alleger le payload viz (gzip + json compact)

Doctrine : "tout doit etre instantane". Le cout dominant de `/api/viz/subgraph`
et `/api/query` est le PAYLOAD JSON (0,75 s pour 43,7 Mo mesure par l orchestrateur
sur runtime-dev vivant ; temps ~ proportionnel a la taille). Deux leviers.

## Mesures de reference (banc = copie de runtime-full-v2, 259164 noeuds)

Centre a gros voisinage : `session-00cedcd9-...` (7053 voisins sortants).

| requete | pretty (avant) | gzip-9 fil | compact | compact+gzip |
|---|---|---|---|---|
| subgraph hops=1 limit=200000 | 3 288 571 o | 96 057 o (34,2x) | 2 122 095 o (64,5%) | 92 728 o |
| subgraph hops=2 limit=200000 | 5 647 977 o | 179 666 o (31,4x) | 3 682 291 o (65,2%) | 179 051 o |

Constat : gzip donne ~31-34x sur le fil (bien au-dela de la cible ~10x). Le JSON
compact retire ~35% de la taille NON compressee ; sur le fil, une fois gzippe,
le gain marginal du compact est ~3% (gzip mange deja l indentation).

## Cible 1 — compression gzip (le gain universel)

- Negociation `Accept-Encoding: gzip` -> reponse `Content-Encoding: gzip`.
- Applique a TOUTES les reponses passant par `respond_cache` (donc subgraph,
  query, overview, modules, etc.) au-dessus d un seuil.
- Crate : `flate2` 1.1.10, DEJA dans le Cargo.lock du workspace (tire par ureq)
  et en cache local -> build hors-ligne, aucune dep lourde ajoutee.
- Garde-fous :
  - seuil : on ne compresse qu au-dessus de 4096 octets (inutile de gziper 200 o) ;
  - types compressibles seulement (json / text/* / xml / javascript) — jamais
    les binaires deja compresses (png, fonts) ;
  - client SANS `Accept-Encoding: gzip` : reponse identite, inchangee ;
  - `Vary: Accept-Encoding` ajoute pour l hygiene de cache HTTP ;
  - le CACHE INTERNE (subgraph/query/overview) stocke toujours la forme NON
    compressee (la cle de cache est inchangee) ; la compression se fait au
    moment de `respond`, par requete, selon l en-tete du client. Aucune
    reponse periemee, aucune collision de cle.

Detail d implementation : le drapeau "ce client accepte gzip" est lu une fois
dans `handle` et depose dans un thread-local (`REQ_ACCEPTS_GZIP`). Le serveur
est un-thread-par-connexion avec `Connection: close` (une reponse par thread),
donc le thread-local est sur et remis a false au debut de chaque requete.

## Cible 2 — allegement du payload par noeud

Analyse de la sortie reelle (banc) : un noeud est deja MINIMAL. Structure
`VizNode` : `id` (numerique court), `qn`, `label?`, `file_path?`, et les
signaux (`energy`, `curvature`, `scc_id`, `community_id`, `kind`), tous
`skip_serializing_if = None`. IL N Y A PAS de sac de proprietes arbitraires a
retirer. Le cout par noeud vient du `qn` (parfois long) et de l indentation.

Verification front (banc-obrain-j5/packages/obrain-dashboard/src) : le type
`GraphNode` declare id/qn/label/file_path + energy/curvature/scc_id/community_id
/engram_id/color_hint/size_hint. Or `Graph3D.tsx` et `CosmosWeave.tsx`
consomment `energy`, `curvature`, `community_id`, `scc_id` dans leurs vues de
BASE (coloration spectrale, taille des hubs par energie), NON conditionnees au
parametre `?overlay`. Droper ces champs (par ex. `signals:false` sans overlay)
CASSERAIT donc le rendu du dashboard. De meme, omettre les valeurs "0.0"
tromperait les tests `!= null` du front (community_id 0 deviendrait "absent").

### Decision (allegement)

Allegement SUR retenu = **JSON compact** (retrait du pretty-print) pour
`/api/viz/subgraph`, via `obrain_viz::to_json_string_compact`. Zero perte de
champ, `JSON.parse` ignore l espace -> non-regression front garantie. Reduit la
taille NON compressee de ~35% (memoire/parse cote navigateur) et permet a plus
de reponses de tenir sous le plafond de cache interne (4 Mo). `/api/query`
serialise deja en compact (`Value::to_string`).

Le mode "leger" qui DROPPE des champs par noeud est **DIFFERE** : il casserait
le front tant qu on n a pas migre Graph3D/CosmosWeave vers un fetch de details
a la demande. Documente ici, non implemente (clause anti-enlisement : livrer le
gain sur sans risque plutot que casser la semantique).

## Preuves

Avant = binaire DEPLOYE (base actuelle prod) servant le banc, port 8481.
Apres = binaire release J7 servant le meme banc, port 8482.
- tailles octets avant/apres (identite / gzip / compact) ;
- temps relatifs plusieurs passes, cache contourne par centres/nonces ;
- identite : reponse gzip decompressee == reponse identite ; champs front tous
  presents ; `/api/query` count/ORDER BY/voisinage inchanges ;
- `cargo test --release -p obrain-cli` vert (dont viz_extract).
