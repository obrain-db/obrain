# RAPATRIEMENT DES OPÉRATIONS NOMMÉES (chantier NANO) — 2026-09-15

*Lignée servie `feat/principal-natif-j5`. Ce document dit ce qui a été rapatrié,
pourquoi, ce que ça donne **mesuré**, et ce qui n'a PAS été rapatrié.*

## 1. Ce qui manquait

Le moteur tient des **compteurs vivants** (`node_count`, `edge_count`,
`node_count_by_label`, `all_labels`) : ce sont des lectures atomiques. La lignée
`perf/chemin-nanoseconde` (09-11) en avait tiré une opération nommée —
`label_counts()` — et avait réécrit l'`overview` du viz pour l'utiliser :

| requête d'origine (GQL) | opération nommée | coût mesuré (banc `chemin_nanoseconde`) |
|---|---|---|
| `MATCH (n) RETURN count(n)` | `db.node_count()` | 1,48 µs → **3,34 ns** |
| `MATCH ()-[r]->() RETURN count(r)` | `db.edge_count()` | 1,71 µs → **3,43 ns** |
| `MATCH (n) RETURN labels(n)[0] AS t, count(n) … LIMIT 14` | `db.label_counts()` | **77,1 ms → 1,89 µs** |

Les deux premiers comptes étaient **déjà O(1)** sous l'enveloppe GQL :
l'enveloppe était tout le coût. Le troisième était un **vrai balayage** des
nœuds — c'est lui qui portait les 77 ms.

Or **ni la lignée servie ni le tronc n'avaient `label_counts()`**, et les deux
faisaient encore les trois requêtes GQL dans `api_overview`.

## 2. Ce qui a été rapatrié

1. `crates/obrain-engine/src/database/admin.rs` : **`label_counts()`** —
   `all_labels()` puis un load atomique par étiquette, trié par compte
   décroissant puis par nom (ordre déterministe).
2. `crates/obrain-cli/src/viz_serve.rs::api_overview` : les trois requêtes GQL
   remplacées par `db.node_count()`, `db.edge_count()`, `db.label_counts()`,
   plus un champ **`by_label_semantique`** qui rend le changement de sémantique
   lisible par le client au lieu de le cacher.

Rien d'autre n'a été porté : les compteurs du substrat sont **identiques** entre
la lignée NANO et la lignée servie (comparaison fonction par fonction de
`store.rs` : `node_count_by_label`, `all_labels`, `live_count_by_edge_type` sont
les mêmes). Les +559 lignes du commit « compteurs » sont de la **documentation
et des tests**, pas un correctif à porter.

## 3. Effet mesuré, sur le service vivant

`/api/viz/overview` sur `runtime-prod` (316 511 nœuds, 1 118 170 arêtes), **cache
vidé des deux côtés** :

| | 1er appel (froid) | 2e | 3e |
|---|---|---|---|
| **avant** (binaire servi) | **165,4 ms** | 0,82 ms | 0,52 ms |
| **après** (code rapatrié) | **0,6 ms** | 0,40 ms | 0,34 ms |

**×276 à froid**, à valeurs identiques : mêmes nœuds, mêmes arêtes, même
histogramme (`Message 46 275`, `ToolResult 45 521`, `ToolCall 45 519`, `Step 42 683`).

## 4. La sémantique CHANGE, et c'est une amélioration mesurée

`labels(n)[0]` ne rend pas « la première étiquette » du nœud : le substrat range
les étiquettes dans un `u64` de bits, et `labels(n)[0]` rend la **plus anciennement
internée de la base entière** — un compteur d'arrivée, pas une propriété du nœud.
Deux bases contenant les mêmes nœuds, peuplées dans un ordre différent, rendent
donc des `labels(n)[0]` différents.

Mesure, store vivant : 15 nœuds portent `[Task, Node]` — `labels(n)[0]` rend
`Node = 4` là où `node_count_by_label("Node")` rend **18**. Ce ne sont pas deux
approximations de la même quantité : ce sont deux questions, et **seule la
seconde est stable**. Un histogramme de dashboard qui répond « combien de nœuds
portent cette étiquette » est la question que le lecteur se pose ; l'ancienne
réponse dépendait de l'ordre d'insertion.

`label_counts()` compte un nœud dans **chacune** de ses étiquettes : la somme de
`by_label` peut donc dépasser `nodes`. Le champ `by_label_semantique`
(`"noeuds_portant_l_etiquette"`) le dit au client.

## 5. Ce qui n'a PAS été rapatrié (et pourquoi)

- **`edge_counts_by_type()`** : balaie les arêtes (O(#arêtes), 12,8 ms mesurés) —
  et les compteurs par type avaient été **mesurés faux d'un facteur deux** dans
  la lignée NANO. Le balayage y est conservé délibérément. Ne pas l'exposer.
- **Le reste de la lignée** : sa base commune avec le tronc remonte au 10/09 et
  elle porte 33 fichiers / 5 130 lignes (zero-copy, vues, heap). Ce n'est pas un
  merge anodin ; on a rapatrié **le geste**, pas la branche.
- **Les requêtes GQL arbitraires** (`/api/query`) restent des balayages : les
  réécrire est le travail du planificateur, pas de ce rapatriement. Mesuré :
  `GROUP BY` label 89 ms, `GROUP BY` type d'arête 455 ms, `CONTAINS 'session-'`
  954 ms — inchangés.

## 6. Vérifier

```bash
# Avant/après, cache isolé (l'ancien binaire est la copie déployée) :
OBRAIN_VIZ_CACHE_DIR=/tmp/vizcache-before ~/.local/share/obrain-harness/bin/obrain \
  viz serve --host 127.0.0.1 --port 8434 --enrich-ricci 0 --preset "Runtime prod=<store>" &
OBRAIN_VIZ_CACHE_DIR=/tmp/vizcache-after ./target/release/obrain \
  viz serve --host 127.0.0.1 --port 8435 --enrich-ricci 0 --preset "Runtime prod=<store>" &
curl -s "http://127.0.0.1:8434/api/viz/overview?store=<store>" -o /dev/null -w '%{time_total}\n'
curl -s "http://127.0.0.1:8435/api/viz/overview?store=<store>" -o /dev/null -w '%{time_total}\n'
```

⚠ **DÉPLOYÉ le 2026-09-15 à 11:15** — voir §7.

## 7. Déploiement (fait) et retour arrière

Le binaire a été installé et le service redémarré **avec sauvegardes préalables** :

| | |
|---|---|
| binaire en service avant | `~/sauvegardes-bascule-v3/obrain.avant-nano-20260915` (md5 `1610aad9…`, extrait de `/proc/<pid>/exe` — donc **exactement** ce qui tournait) |
| binaire installé | `banc-obrain-j5/target/release/obrain` **et** la copie déployée `~/.local/share/obrain-harness/bin/obrain` (md5 `fccbd880…`, 159 633 752 o) |
| features | vérifiées **identiques** entre l'ancien et le nouveau (`ai` présent, `IAM NON COMPILÉ` absent des deux, `centre introuvable` présent) |
| redémarrage | `systemctl --user restart obrain-viz` — PID 325285, actif à 11:15:33 |

**Vérifications sur le service vivant (8432)** :

| Contrôle | Résultat |
|---|---|
| `ui-verifier.sh` (ExecStartPost, au démarrage) | **`UI SERVIE CONFORME`** — le contrôle du magasin de versions a parlé tout seul dans le journal |
| `/api/viz/overview` cache `ov` **vidé**, runtime-prod | **0,617 ms** (2e : 0,524 · 3e : 0,409) contre **165,4 ms** avant — mêmes valeurs (316 511 / 1 118 170), plus `by_label_semantique` |
| transport WS (`/api/viz/subgraph/stream`) | 15 nœuds / 14 arêtes, frames `begin nodes edges end` |
| `/api/stores`, `/api/viz/modules`, `/api/viz/status` | 200 |
| batterie `/api/query` (8 formes) | **identique** à avant le port — le port ne touche que `api_overview` |

**Retour arrière, en une commande** :

```bash
install -m 755 ~/sauvegardes-bascule-v3/obrain.avant-nano-20260915 \
                 ~/.local/share/obrain-harness/bin/obrain
cp ~/sauvegardes-bascule-v3/obrain.avant-nano-20260915 \
   ~/projects/banc-obrain-j5/target/release/obrain
systemctl --user restart obrain-viz.service
```

