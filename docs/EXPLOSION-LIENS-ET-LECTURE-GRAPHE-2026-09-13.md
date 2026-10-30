# Explosion des liens et lecture du graphe — ce qui a changé, et pourquoi

Trace de revue, 2026-09-13. Tout ce qui suit est **mesuré** sur `runtime-dev`
(≈ 408 000 nœuds, 1 255 000 arêtes) et sur `runtime.obrain`, sauf mention
contraire. Les chiffres « avant » sont ceux relevés avant correctif, « après »
ceux du binaire courant.

## En une ligne

La lecture d'un voisinage est passée de **~900 ms à ~0 ms** ; deux écritures de
la même requête à 2 sauts qui différaient de **154×** donnent maintenant le même
temps ; et trois requêtes qui **faisaient mourir ou pendre** le serveur (produit
cartésien, longueur variable, requête sans fin) **échouent proprement**, avec un
message qui dit quoi écrire à la place.

## Les cinq défauts corrigés

### 1. Le plafond de génération mentait (`engine.rs`)

`SORTIE_MINIMALE = 512` servait à la fois de **réservation de fenêtre** (juste)
et de **plancher du plafond de génération** (faux) : `?max_tokens=24` produisait
jusqu'à 512 jetons.

| requête `ask` | avant | après |
|---|---|---|
| `max_tokens=24` | 50 128 ms | **2 803 ms** |
| `max_tokens=400` | ~50 000 ms | **13 539 ms** |

Corrigé en séparant les deux notions (`plafond_generation`). La réponse dit
désormais le plafond réellement servi (`max_tokens_effectif`, `plafond_atteint`,
`jetons_sortie`, `raisonnement_demande`, `moteur_decompose`).

**Levier de pensée** : sur ce modèle, la pensée consomme tout le plafond avant la
réponse. `chat_template_kwargs.enable_thinking=false` donne **43 481 → 4 248 ms**
avec une vraie réponse française. La route `ask` (bloquante, **sans boucle
d'outil**) la coupe par défaut ; le chat en flux la garde (ses outils en
dépendent). `?raisonnement=1` la rétablit.

### 2. Deux écritures de la même requête à 154× (`filter.rs`)

```text
Filter (id(b) In [3])            ← ne peut pas descendre sous l'expansion
  Join (Inner)                       → 31 724 ms
    Expand(a)->(b) ← NodeScan(a)
    Expand(b)->(c) ← NodeScan(b)
```

Le prédicat est maintenant **poussé dans chaque côté** qui porte ses variables —
chaque côté devient un motif à cible contrainte, et le retournement ci-dessous
s'y applique. **31 724 → 194 ms**, mêmes 19 968 lignes. À 3 sauts : 608 ms pour
1 070 976 lignes.

### 3. La lecture entrante coûtait 900 ms (`filter.rs`, `store.rs`)

`WHERE id(b) IN […]` retombait sur un **scan complet de la table des arêtes**,
alors que `WHERE id(a) IN […]` (source) était poussé sous le scan : **850 ms
contre 85 µs**. La même contrainte, d'un côté ou de l'autre du motif.

Le motif est désormais **retourné** quand la cible est contrainte : la cible
devient la graine (le seek existant s'applique) et la traversée se fait en
**entrant** (`incoming_heads` + chaîne `next_to`), en **O(degré)**.
Sémantique inchangée : l'ensemble des triplets `(a, r, b)` vérifiant
`(b)<-[r]-(a)` est exactement celui qui vérifie `(a)-[r]->(b)`.

| | avant | après |
|---|---|---|
| phase d'expansion d'une question | 905 027 µs | **39–282 µs** |
| moteur d'une question | 878–891 ms | **0 ms** |
| requête entrante seule (23 696 lignes) | 216–923 ms | **6,8–47 ms** |
| `/api/viz/subgraph?inbound=1` | 851,7 ms | **0,5 ms** (99 ms au 1er appel) |

Deux correctifs de substrat l'ont rendu possible : la **détection de cycle** au
premier maillon revu (le garde valait ~1,25 M de pas ⇒ **145 ms et ~50 Mio
collectés par nœud fautif**, cinq nœuds cycliques sur `runtime-dev`), et la
confirmation que les chaînes sont cohérentes sur **11 instances de bases /
33 nœuds / 66 comparaisons** contre le plan générique.

**Et la capacité est désormais CONSTATÉE, pas affirmée** (`has_backward_adjacency`
renvoyait `true` en dur). Le substrat vérifie, par échantillon déterministe
(64 destinations, une fois par instance), que la chaîne entrante rend exactement
ce que le balayage des arêtes compte ; le planificateur ne retourne le motif que
sur une capacité **constatée**, sinon il retombe sur le plan générique.

| | capacité constatée | refus (`OBRAIN_VERIF_ADJACENCE=refus`) |
|---|---|---|
| verdict | `[flip] RETOURNÉ` | `[flip] adjacence entrante non vérifiée sur cette base` |
| coût | **0,4–0,6 ms** (1re : 139 ms, vérification incluse) | **373–418 ms** (plan générique) |
| résultat | 23 696 | **identique** |

Un test casse délibérément une chaîne `next_to` : le verdict devient `false`, et
la chaîne rend alors 1 arête sur les 2 comptées — c'est précisément le résultat
faux et silencieux que cette vérification empêche.

### 4. Le garde du produit cartésien (`join.rs`, `mod.rs`)

```text
NodeScan (c:*)          ← un scan complet de 407 677 nœuds…
  Expand (a)->(b)       ← …pour chacune des 1 255 485 arêtes
```
≈ **5·10¹¹ paires** ⇒ le serveur **mourait**, sans message (OOM). Le planificateur
refuse désormais ce qui est **prouvé** énorme, avec les bornes et trois remèdes :

> `scan imbriqué (produit cartésien) refusé : au moins 1254076 × 407677 =
> 511257941452 paires (plafond 1000000000). … Reliez les parties par une variable
> commune, bornez par un LIMIT, ou comptez autrement.`

Précision vérifiée : une jointure par **égalité corrélée** (que le moteur sait
faire par sonde d'index) **n'est pas refusée** (0,5 s → 407 900), et une jointure
modeste passe toujours.

### 5. Le pouvoir d'arrêt n'était armé nulle part

Le moteur a une échéance de requête complète et testée — et le code le dit
lui-même (`common/utils/cancel.rs` : « *Complète, testée… et jamais armée en
production* »). Mesuré : `MATCH (a)-[*1..2]->(b) WHERE id(b) IN [315447] …`
tournait **encore après 150 s**.

Armée désormais aux **trois points d'entrée** :

| point d'entrée | défaut | comportement observé |
|---|---|---|
| `viz serve` | 30 s | HTTP 400 « Query exceeded timeout », serveur vivant |
| `obrain query` (CLI) | 60 s | code de sortie **1** + message |
| pont MCP (lecture seule) | 30 s | erreur JSON-RPC au délai, au lieu d'un appel d'outil qui ne rend jamais la main |

Le pont **n'arme pas** le chemin d'écriture (`OBRAIN_WRITE=1`) : une écriture
longue (indexation, import par lots) est légitime et ne doit pas être coupée.

## Les trois réglages

| variable | défaut | effet |
|---|---|---|
| `OBRAIN_QUERY_TIMEOUT_MS` | 30 000 (serveur, pont) · 60 000 (CLI) | échéance des requêtes ; `0` désarme |
| `OBRAIN_MAX_PRODUIT` | 1 000 000 000 | plafond des paires d'un produit prouvé ; `0` désarme |
| `OBRAIN_VERIF_ADJACENCE` | vérification active | `0` la saute (bancs) · `refus` force le repli (mesure ce que le retournement apporte) |
| `?raisonnement=1` (route `ask`) | coupée | rétablit la pensée du modèle |

## Construction — le piège

`scripts/vizserve.sh` lance `./target/release/obrain` **tel quel**, et une
reconstruction ordinaire l'appauvrit en silence :

```bash
cargo build --release -p obrain-cli --features ai     # ← la seule commande correcte
```

`ai = ["vector-index", "text-index", "hybrid-search", "cdc"]`. Un build sans
`--features ai` **retire ces quatre features du binaire servi**. C'est arrivé
pendant ce chantier ; l'artefact a été reconstruit avec la bonne commande
(symboles `vector_index` / `text_index` / `cdc_` vérifiés présents).

## Ce qui est vérifié

- **Tests** : moteur **700/0**, CLI **13/0** (`engine`) et **11/0** (`viz_extract`),
  substrat **19/0** (chaînes).
- **Balayages d'endpoints** : **27/27**.
- **Résultats** : batterie de 11 requêtes entrantes — **ensembles de lignes
  identiques** à la référence figée avant correctif (seul l'ordre des ex æquo
  sous `ORDER BY` bouge) ; cas tordus (conjonction d'id, type d'arête, source
  labellisée, 2 sauts contraints au milieu) tous identiques.
- **Sondes de contrôle** : produit refusé, jointure indexée non refusée, délai
  effectif aux trois points d'entrée, code de sortie du CLI.

## Ce qui reste ouvert (assumé, non corrigé ici)

- `-[r*N]->` (**arête liée** sur une longueur variable) n'est pas retourné :
  l'ordre de la liste s'inverserait — le compte serait juste, la liste fausse.
  Borné en *temps* par l'échéance, pas optimisé ; le correctif est d'inverser la
  liste dans l'opérateur (`variable_length_expand.rs`).
- Le **plafond de nombre de chemins** appartient à l'opérateur (sa vigie
  d'annulation borne la mémoire par lots de 256 ; l'échéance borne le temps).
  Un motif à 4,89 M de chemins reste légitime (38 s) : c'est la *réponse* qui est
  énorme, pas le plan.
- Le **budget mémoire** (`config.memory_limit`) n'est consulté par aucun
  opérateur d'exécution ; le garde du produit est un substitut de planificateur,
  précis mais pas un budget.
- Les **compteurs ne sont pas validés par la lisibilité** : `linux-full3` annonce
  1 553 071 nœuds / 4 779 754 arêtes et **ne rend aucune ligne**.
- Le REPL (`obrain shell`) n'est pas armé — un humain peut interrompre.
