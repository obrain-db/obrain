# ENGINE008_NEXTN_OBSERVABLE_SIGNATURES

**Passe :** SUBMITTED-GRAPH-AUDIT-01

## 1. Signatures NextN dans les sources Q1 — et leur représentation

| Signature C++ | Emplacement | Représentation dans le graphe | Représentation dans une surface f1a |
|---|---|---|---|
| `t_h_nextn` | `llama-graph.cpp:1271`, `:1317-1318` (`ggml_set_output`) | tenseur d'état NextN | **AUCUNE** |
| `n_layer_nextn` | `llama-graph.cpp:1407` (hyperparamètre) | n'entre pas dans le graphe | **AUCUNE** |
| `LLM_TENSOR_NEXTN_EH_PROJ`, `..._SHARED_HEAD_HEAD` | `llama-model.cpp:1476-1480`, `:1541-1545` | création de tenseurs de poids | **AUCUNE** |
| tenseurs `blk.64.nextn.*` | métadonnée GGUF du candidat (4 tenseurs) | niveau **modèle**, pas graphe | **AUCUNE** (aucune surface f1a ne lit les tenseurs du modèle) |

## 2. Vérification décisive

```text
$ grep -n "nextn" src/f1a-capture.cpp
(vide)
```

**L'instrumentation f1a n'instrumente pas le chemin NextN.** Aucun tag, aucune
étiquette, aucun champ NextN n'est émis — ni dans `SUBMITTED_GRAPH_SNAPSHOTS.csv`,
ni dans `GRAPH_ROLE_RELATIONSHIPS.csv` (surface WHT), ni ailleurs.

Les neuf surfaces produites sont : `EVENT_LEDGER`, `GRAPH_CAPTURE_MANIFEST`,
`GRAPH_ROLE_RELATIONSHIPS`, `TARGET_EFFECT_PATH`, `PAYLOAD_PROVENANCE`,
`BACKEND_OWNERSHIP_MANIFEST`, `RESIDENCY_MANIFEST`, `EPOCH_CLASSIFICATION`,
`SUBMITTED_GRAPH_SNAPSHOTS`. **Aucune ne porte d'identité d'opération.**

## 3. Règle de classification qui en découle

Une classification NextN exige une surface qui **énumère les opérations** du
graphe soumis, ou qui porte un tag d'opération dont la grammaire est établie au
producteur. **Aucune des deux n'existe dans Q1 tel qu'il est.**

Par conséquent :

- **Interdit** : rechercher des sous-chaînes (`nextn`, `NEXTN`, `h_nextn`) —
  c'était l'ancien chemin, invalidé.
- **Interdit** : déduire NextN de `n_nodes` ou d'un `fingerprint` — un volume
  n'est pas un contenu.
- **Seule issue** : une surface native qui énumère les nœuds. Elle n'existe pas
  dans Q1, et cette passe **n'ajoute pas d'instrumentation** au calcul.

## 4. Statut des quatre niveaux

| Niveau | Statut |
|---|---|
| tenseurs NextN **présents** (métadonnée GGUF) | établi, mesuré au niveau modèle |
| tenseurs NextN **chargés** | non observable sans surface de chargement |
| opérations NextN **construites** dans le graphe | **NON OBSERVABLE** — aucune surface n'énumère les nœuds |
| opérations NextN **exécutées** | NON OBSERVABLE, et de toute façon non exercé |

Le niveau 3 est **le maillon bloquant**. Aucun résultat négatif ne peut être
publié sur NextN avant qu'une surface native l'expose.
