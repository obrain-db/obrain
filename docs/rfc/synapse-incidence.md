# ADR — SynapseIncidence : l'index cognitif persistant (décommission de la DashMap du chemin de lecture)

Date : 2026-08-31. Statut : **PROPOSÉ** (design, à implémenter par tranches).

## 1. Le constat

`SynapseStore` garde une `DashMap` comme « warm-read accelerator » (T6).
Les lectures du spread (`for_each_synapse`) passent par :
- `node_to_keys` (DashMap en mémoire) → poids via la colonne substrate ;
- rechargée à chaque boot par `load_from_substrate_column` (scan O(E)).

La DashMap **n'est pas une dette** : le substrat n'a pas d'index inversé
persistant `node → synapses`, donc le spread a besoin d'une structure en
mémoire pour être O(degré). La décommission évoquée (props v2, 2026-08-19)
concernait l'écriture double des propriétés, résolue par PropsZone — pas le
cognitif.

## 2. La nouvelle forme : `SynapseIncidence`

Le substrat a DÉJÀ le pattern exact : `SimplexIncidence` (répertoire plat
16 B par slot de nœud → têtes de postings 4 KiB, lecture O(degré simplicial)).
On le réutilise pour les synapses :

```
substrate.synapse_incidence   (répertoire 16 B × node_slot)
substrate.synapse_postings    (pages 4 KiB de EdgeId, append-only)
```

- `create_synapse_slot(src, dst, w)` ajoute `eid` au postings de `src` (et
  `dst` si direction both) — écriture O(1) amorti, WAL-loguée comme les
  autres zones.
- `for_each_synapse(node)` = marche du postings de `node` → EdgeId → poids
  colonne — **O(degré), zéro DashMap, zéro scan O(E)**.
- `load_from_substrate_column` devient inutile au boot : la structure EST
  la source (pas de réhydratation).

## 3. Ce que ça décommissionne

- `node_to_keys` (DashMap) sort du chemin de lecture du spread.
- `load_from_substrate_column` (le scan O(E) au boot que j'ai ajouté) n'est
  plus nécessaire — la persistance est native.
- La DashMap reste uniquement comme cache de poids chaud optionnel (ou est
  retirée entièrement dans une tranche suivante).

## 4. Tranches d'implémentation

1. **T1** : zone `SynapseIncidence` (répertoire + postings), écrite par
   `create_synapse_slot`, lue par un nouveau `for_each_synapse_substrate`.
2. **T2** : bascule `for_each_synapse` sur l'incidence ; garder DashMap en
   lecture seule (parité de résultats vérifiée par test).
3. **T3** : retirer `node_to_keys` + `load_from_substrate_column` du chemin
   chaud ; garder les tests de parité.

## 5. Vérification

- Parité : spread avant/après (mêmes nœuds activés, mêmes énergies).
- Perf : `for_each_synapse` O(degré) sans scan O(E) — bench sur harness
  (29 994 synapses, degré moyen 8) : cible < 1 µs par nœud.
- Persistance : bootstrap → réouverture → spread propage (déjà vérifié,
  doit rester vrai SANS `load_from_substrate_column`).
- Non-régression : 27/27 cognitif, 771/771 substrate.

## 6. Références

- `SimplexIncidence` (`crates/obrain-substrate/src/simplex_incidence.rs`) —
  le pattern de répertoire plat + postings.
- `create_synapse_slot` (store.rs) — l'écriture synapse actuelle.
- `for_each_synapse` (synapse.rs) — le chemin de lecture à basculer.
