# La charge `json-graph` du moteur de rendu — mesurée, et ce qui reste

Date : 2026-09-15. Store : `runtime-dev` (439 688 nœuds / 1 324 105 arêtes).
Route : `GET /api/viz/subgraph?center=adr:dsh-runtime/architecture/relief-complet-et-propagation&hops=1&limit=500`.

Ce document existe pour qu'on puisse **reprendre** le travail sur la charge de
rendu sans refaire les mesures, et pour que le gain déjà obtenu ne soit pas
re-confondu avec le gain qui reste.

## 1. Ce que le moteur de rendu reçoit

| mesure | valeur |
|---|---|
| charge non compressée | **2 314 165 octets** (avant correction) |
| même charge en gzip | 79 458 octets |
| 1re requête (froid) | 405 ms |
| requête rejouée (cache 30 s, `Arc<str>`) | 53 ms |
| contenu | 500 nœuds, **24 502 arêtes** |

Le sous-graphe d'un seul centre, à `hops=1`, fait donc 2,3 Mo — et c'est le
client (le moteur de rendu) qui paie l'essentiel : il doit **parser** ces 2,3 Mo
de JSON. Le serveur, lui, est déjà bon : cache `Arc<str>` sans copie d'octets
(`SUBGRAPH_CACHE`, TTL 30 s), gzip ×29.

## 2. Correction faite : les signaux f32 émis en double précision

`VizEdge.ricci` et `VizNode.curvature` sont des `Option<f64>` dans l'IR, mais
leurs valeurs **naissent en f32** :

- `record.rs` : `(self.ricci_u8 as f32 / 127.5) - 1.0` — la source est un octet,
  la valeur est f32 par construction ;
- `viz_extract.rs` : `node_curvature(store, ..).map(f64::from)` — un f32 élargi.

Sérialisés en f64, ils sortaient sur 17 chiffres : `-0.8352941274642944`, dont
les dix derniers ne portent **rien** que la source ne contienne déjà. `ricci`
pesait à lui seul **454 944 octets** (18,6 par arête).

L'IR émet désormais la forme courte du **même f32** (`f32_court`, `ir.rs`) :

| | avant | après |
|---|---|---|
| `ricci` (24 502 arêtes) | 454 944 o | ~247 800 o |
| `curvature` (500 nœuds) | 9 500 o | ~5 001 o |
| **charge totale** | **2 314 165 o** | **2 102 520 o** (−9,1 %) |

**Perte nulle, prouvée et non supposée** : `serialize_f32` écrit la
représentation la plus courte qui rejoue exactement le même f32 (ryu). Sur les
valeurs réelles de la charge, `float32(forme courte) == float32(original)` pour
**24 502 / 24 502** arêtes et **500 / 500** nœuds, 0 divergence. L'ensemble des
arêtes est inchangé (mêmes `src`/`dst`/`type`).

`energy` n'a **pas** été touchée : elle vient d'un Q1.15 et son origine f32
n'est pas établie. On ne raccourcit que ce qu'on a vérifié.

### Contrepartie, assumée et mesurée

Le **gzip est très légèrement plus gros** : 79 458 → 80 567 octets (+1,1 Ko),
parce que la répétition des chiffres se compressait presque gratuitement. Le
transfert ne change donc pas de façon perceptible ; ce qui baisse vraiment, de
9 %, est la charge que le rendu **parse**. Le gain est côté client, pas côté
fil — ne pas le présenter comme un gain de bande passante.

## 3. Ce qui reste : la redondance STRUCTURELLE (~70 % de la charge)

C'est le vrai poste, et il n'est pas traité. Composition mesurée :

| champ d'arête | octets | valeurs **uniques** |
|---|---|---|
| `ricci` | 454 944 | 24 502 |
| `type` | 269 562 | **3** |
| `src` | 196 016 | **1** |
| `dst` | 196 016 | 24 502 |
| `weight` | 73 506 | — |

Deux gaspillages que la correction ci-dessus ne touche pas :

1. **Des chaînes répétées à l'identique** — `type` prend 3 valeurs pour 24 502
   arêtes (270 Ko), et `src` **une seule** (196 Ko).
2. **L'ossature JSON elle-même** — les noms de clés, guillemets, accolades et
   virgules pèsent à peu près autant que les valeurs (~1,1 Mo des 2,3 Mo).

### Encodage proposé (à faire, non fait ici)

Un encodage **colonnaire** plutôt qu'une liste d'objets :

```json
{
  "schema": "obrain/json-graph@2",
  "nodes": { "id": [...], "curvature": [...], "kind": [...] },
  "edges": { "src": [...], "dst_idx": [...], "type_idx": [...], "ricci": [...] },
  "strings": ["CALLS", "IMPORTS", "CONTAINS"]
}
```

Attendu, à vérifier par la mesure : supprimer les noms de clés répétés et
remplacer `type`/`src` par des indices de table ramène la charge sous **800 Ko**
(ordre de grandeur à établir, pas à croire).

### Pourquoi ça n'a PAS été fait ici

Ce n'est pas une optimisation locale : **c'est un changement de contrat de
fil**. Le client (`packages/obrain-dashboard`, `lib/api.ts` → `JsonGraph`) doit
être modifié en même temps, sinon le rendu casse. Or le dashboard était en
cours de modification par une autre session au moment de cette mesure
(commits `feat(ui): …` du 2026-09-15). Une optimisation serveur seule aurait
cassé l'écran.

**Étapes pour finir :**

1. geler le format `@2` (colonnaire, table de chaînes) et l'annoncer dans
   `schema` ;
2. émettre `@2` **en plus** de `@1` derrière un paramètre
   (`?schema=2`), pour que les deux clients coexistent pendant la bascule ;
3. faire lire `@2` par `lib/api.ts` (une passe de décodage : indices → chaînes) ;
4. mesurer avec la méthode du §4, comparer, puis retirer `@1` quand le
   dashboard n'en dépend plus.

## 4. Comment reproduire ces mesures

```bash
# serveur isolé (sans IAM), cache neuf
OBRAIN_VIZ_CACHE_DIR=/tmp/viz-rendu OBRAIN_PRECHO_ARCS_MS=0 \
  target/release/obrain viz serve --host 127.0.0.1 --port 8436 --enrich-ricci 0

S='%2Fhome%2Ftriviere%2F.local%2Fshare%2Fobrain-harness%2Fdata%2Fruntime-dev.obrain'
C='adr:dsh-runtime/architecture/relief-complet-et-propagation'

# charge non compressée / compressée
curl -s -o /tmp/sg.json -w "%{time_total} %{size_download}\n" \
  "http://127.0.0.1:8436/api/viz/subgraph?store=$S&center=$C&hops=1&limit=500"
curl -s -o /dev/null -H 'Accept-Encoding: gzip' -w "%{time_total} %{size_download}\n" \
  "http://127.0.0.1:8436/api/viz/subgraph?store=$S&center=$C&hops=1&limit=500"
```

Puis, pour l'égalité au f32 près entre deux charges (ancienne / nouvelle) :
comparer `float32(forme courte) == float32(original)` valeur par valeur, arêtes
appariées par `(src, dst, type)` après tri — c'est ce qui distingue « plus
court » de « différent ».

**Piège à ne pas retomber dedans** : mesurer une route authentifiée sans cookie
rend un **401 de 480 octets en 0,5 ms** — un temps excellent qui ne mesure
rien. L'IAM est armé depuis le 2026-09-15 ; vérifier `%{http_code}`, pas
seulement `%{time_total}`.
