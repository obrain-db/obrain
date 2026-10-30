# La charge de `/api/viz/subgraph` — ce qu'elle pèse, ce qui a été gagné

Mesures du 2026-09-15, service isolé (`viz serve` sur `127.0.0.1`, binaire de
`consolidation/tronc-release-2026-09-15`), store `runtime-dev`
(439 688 nœuds / 1 324 105 arêtes).

Requête de référence :

```
GET /api/viz/subgraph?store=…/runtime-dev.obrain
    &center=adr:dsh-runtime/architecture/relief-complet-et-propagation
    &hops=1&limit=500
```

## 1. L'état mesuré

| mesure | valeur |
|---|---|
| charge v1 (identité) | **2 102 520 octets** |
| charge v1 (gzip) | 80 567 octets |
| charge v2 (identité) | **891 083 octets** |
| charge v2 (gzip) | 125 799 octets |
| 1ʳᵉ requête (calcul froid) | 405 ms |
| requête rejouée (cache `SUBGRAPH_TTL` 30 s) | 53 ms |
| `JSON.parse` V8 — v1 | 10,1 ms (médiane de 7) |
| `JSON.parse` V8 — v2 | **3,5 ms** |

Contenu : **500 nœuds et 24 502 arêtes** — ~49 arêtes par nœud. Les arêtes
portent 96 % du poids.

## 2. Où passe chaque octet (mesuré, pas estimé)

Valeurs seules, par champ, sur les 24 502 arêtes :

| champ | octets | valeurs **uniques** |
|---|---|---|
| `ricci` | 454 944 | 24 502 |
| `type` | 269 562 | **3** |
| `src` | 196 016 | **1** |
| `dst` | 196 016 | 24 502 |
| `weight` | 73 506 | — |

Nœuds (500) : `qn` 12 535 · `curvature` 9 500 · `kind` 5 495 · `id` 4 000 ·
`energy` 1 500 · `scc_id` 1 390 · `community_id` 500.

Le reste de la charge (~1,0 Mo) est de la **structure** : noms de clés,
guillemets, accolades, virgules. Chaque arête répète `"src"`, `"dst"`, `"type"`,
`"weight"`, `"ricci"` — 21 caractères de noms de clés pour ~40 octets de
valeurs. Et **`src` n'a qu'une seule valeur distincte** sur 24 502 arêtes.

## 3. Gagné — palier 1 : les signaux f32 (−9,1 %)

`ricci` naît en f32 (`obrain-substrate/src/record.rs` :
`(self.ricci_u8 as f32 / 127.5) - 1.0`) et `curvature` aussi
(`viz_extract.rs` : `node_curvature(...).map(f64::from)`). Les champs de l'IR
étant des `f64`, ils étaient sérialisés en **double précision** :
`-0.8352941274642944` — dix-sept chiffres dont dix ne portaient rien que l'octet
`ricci_u8` ne disait déjà.

`ir.rs` émet donc ces deux champs avec `serialize_f32` (forme la plus courte qui
rejoue le **même** f32, via ryu) : **2 314 165 → 2 102 520 octets (−9,1 %)**.
Perte nulle, vérifiée : `float32(forme courte) == float32(original)` pour
**24 502/24 502** arêtes et **500/500** nœuds.

`energy` n'est pas touchée : elle vient d'un Q1.15, son origine f32 n'est pas
établie. On ne raccourcit que ce qu'on a vérifié.

## 4. Gagné — palier 2 : la v2 colonnaire (−58 % d'octets, −65 % de parse)

`json_graph_v2.rs` : les chaînes sont écrites **une fois** dans `strings`, et
chaque arête n'est plus qu'un **indice**. Une colonne optionnelle est OMISE si
aucune arête ne la porte, et un trou dans une colonne reste un trou (`null`) —
c'est le piège classique de ce type d'encodage, il est verrouillé par test.

| | v1 | v2 |
|---|---|---|
| octets (identité) | 2 102 520 | **891 083** (−58 %) |
| octets (gzip) | **80 567** | 125 799 (**+56 %**) |
| `JSON.parse` V8 (médiane de 7) | 10,1 ms | **3,5 ms** (−65 %) |

**Le gzip s'inverse, et c'est le résultat le plus important de cette note.** Le
gzip adore la redondance de la v1 : plus la charge est dense, moins il peut la
comprimer. Sur un lien lent, la v1+gzip reste donc MEILLEURE en octets
transmis ; sur le rendu, la v2 est 2,9× plus rapide à parser.

Ce que ça implique, selon la topologie :

* **dashboard servi en LAN** (le cas ici, `192.168.1.103`) : le transfert est
  quasi gratuit, le `JSON.parse` domine → **la v2 gagne** ;
* **accès à distance sur lien faible** : la v1+gzip gagne en octets, mais on
  paie 6,6 ms de parse en plus à chaque vue.

**Décision non prise ici** : la v2 est servie **uniquement sur demande
explicite** (`?format=json-graph-2`). Le défaut est la v1, inchangée à l'octet
(vérifié : 2 102 520 des deux côtés). Basculer le défaut est un choix de
déploiement, pas de performance — il appartient à qui connaît les clients.

### Ce qui a été testé

`cargo test -p obrain-viz` : **76/76** (+10 pour la v2), `json_graph.rs` et les
goldens json/dot/mermaid inchangés. Notamment :

* `v2_restores_the_original_exactly_where_v1_cannot` — la v1 écrit `ricci` en
  forme courte, donc **relire son propre corps ne restitue plus le f64 d'origine**
  (`-0.8352941` au lieu de `-0.8352941274642944`). La v2, elle, restaure l'IR à
  l'identique. Conséquence réelle : la voie WebSocket relit le corps v1 en cache
  (`subgraph_for_stream` → `from_json_str`), elle servait donc des f64 très
  légèrement différents de la voie HTTP. Sans effet sur le rendu (même f32).
* `a_hole_in_an_optional_column_stays_a_hole` — une arête sans `ricci` n'hérite
  pas de sa voisine.
* `a_ragged_column_is_refused_not_silently_truncated` — une colonne plus courte
  est REFUSÉE, jamais tronquée en silence.

## 5. Reste à gagner : les `dst` sont écrites DEUX fois (~270 Ko)

La table `strings` de la v2 compte **24 506 entrées** pour 500 nœuds — parce que
chaque `dst` est un id distinct, et que ces ids sont **déjà** dans
`nodes[].id`. La v2 les stocke une seconde fois.

Encoder `src`/`dst` comme des **indices dans `nodes`** (au lieu d'indices dans
`strings`) supprimerait cette duplication : la table tomberait à ~4 entrées
(3 types + la source unique), soit **~270 Ko de moins** (0,89 → ~0,62 Mo).

Le seul obstacle est identifié et documenté : une arête peut légitimement pendre
hors de la fenêtre (`limit` sur un nœud très connecté — voir
`JsonGraphDoc::into_subgraph_lenient`). Convention proposée :

```
valeur v < nodes.len()  → nodes[v].id
valeur v >= nodes.len() → externals[v - nodes.len()]
```

**À faire seulement si la v2 est adoptée** : rendre la charge encore plus dense
la rend aussi moins compressible, donc ce pas doit être tranché avec la même
mesure à trois colonnes (identité / gzip / parse) que ci-dessus, pas à l'aveugle.

## 6. Piste complémentaire, à trancher par le produit

`limit=500` borne les **nœuds**. Le sous-graphe rend 24 502 arêtes pour 500
nœuds ; sur un centre fortement connecté, la charge croît avec la densité, pas
avec `limit`. Borner aussi les arêtes (par poids décroissant, avec `truncated`
déjà présent) diviserait la charge, mais retire des liens de l'écran : décision
de rendu, pas de performance.
