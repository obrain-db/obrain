# ENGINE008_GRAPH_OBSERVER_SOURCE_MAP

**Passe :** GRAPH-ENUMERATOR-01

## 1. Chaîne complète, du producteur à l'accès

| Maillon | Site exact | Nature |
|---|---|---|
| pointeur du graphe soumis capturé | `f1a-capture.cpp:91` — `f1a_last_gf` | variable globale **exportée** |
| nombre de nœuds | `f1a-capture.cpp:92` — `f1a_last_gf_nodes` | variable globale **exportée** |
| surface de volume | `f1a-capture.cpp:301-310` — `SUBMITTED_GRAPH_SNAPSHOTS.csv` | `submission_id,gf,n_nodes,submission_rc,fingerprint` |
| écriture du `gf` dans le ledger | `f1a-capture.cpp:206-207` | `%p` de `f1a_last_gf`, `NA` si nul |
| helpers de lecture du graphe figé | `f1a-capture.h:70-75` | `f1a_ptr_in_submitted_graph`, `f1a_submitted_graph_output_count`, `f1a_sub_node_src`, `f1a_sub_node_op` |
| énumération ggml publique | `ggml.h:2783`, `:2785` | `ggml_graph_node`, `ggml_graph_n_nodes` |

## 2. Dictionnaire d'opérations

`enum ggml_op op` est un champ **public** de `ggml_tensor` (`ggml.h:695`).
Le nom textuel d'une opération doit passer par `ggml_op_name()`, dont
l'export doit être **confirmé** avant usage — ne pas le supposer.

**Règle conservatrice :** la classification s'appuie sur le **code d'opération
numérique** (`op`) et non sur une chaîne. Le nom n'est qu'un confort de lecture.
C'est la leçon directe de l'ancien classificateur invalidé, qui cherchait des
sous-chaînes là où aucun producteur n'en émettait.

## 3. Signatures NextN → structures observables

| Signature Q1 | Emplacement | Représentation dans le graphe soumis |
|---|---|---|
| `t_h_nextn` | `llama-graph.cpp:1271`, `ggml_set_output` en `:1317-1318` | **un tenseur du graphe**, atteignable par énumération ; marqué `ggml_set_output` |
| tenseurs `blk.64.nextn.*` | métadonnée GGUF + `llama-model.cpp:1476-1480`, `:1541-1545` | apparaissent comme **feuilles** (poids) du graphe |
| `n_layer_nextn` | `llama-graph.cpp:1407` | hyperparamètre : **n'entre pas** dans le graphe |

**Règle conservatrice, énoncée avant toute mesure :** la présence d'un **poids**
NextN ne prouve pas l'exécution d'une **opération** NextN. La classification doit
porter sur les opérations qui **consomment** ces feuilles, pas sur les feuilles.

## 4. Deux pièges identifiés à l'avance

1. **`f1a_last_gf` est écrasé à chaque soumission.** Une énumération hors
   fenêtre lit le mauvais graphe. La corrélation par `submission_id` et la
   confrontation à la colonne `gf` du snapshot sont **obligatoires**.
2. **Le nom d'un tenseur n'est pas une identité.** Un nœud peut porter un nom
   trompeur : le classificateur doit exiger une **structure** (opération
   consommant une feuille NextN), jamais une correspondance de chaîne.
