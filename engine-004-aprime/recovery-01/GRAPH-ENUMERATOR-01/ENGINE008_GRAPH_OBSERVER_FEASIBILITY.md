# ENGINE008_GRAPH_OBSERVER_FEASIBILITY

**Passe :** GRAPH-ENUMERATOR-01 — 2026-10-02
**Verdict :** `GRAPH_OBSERVER_METHOD=EXPORTED_f1a_last_gf_PLUS_PUBLIC_GGML_GRAPH_ACCESSORS`
**`GRAPH_OBSERVER_ACCESS_VERIFIED=YES`** — aucune modification de libllama Q1 requise.

---

## 1. Les trois accès examinés

| # | Accès | Disponible dans Q1 ? | Verdict |
|---|---|---|---|
| 1 | `llama_context_params.cb_eval` | **OUI** (`include/llama.h:369-370`) | utilisable, mais voir §4 |
| 2 | Symboles f1a exportés par `libllama.so` | **OUI** (vérifié par `nm -D`) | **retenu** |
| 3 | Accesseur de graphe sur `llama_context` | **NON** — aucun `get_graph` dans `llama.h` | écarté |

---

## 2. L'accès retenu — vérifié symbole par symbole

`nm -D --defined-only libllama.so.0.0.135` (bibliothèque **Q1**, SHA
`87c42d5f…`) exporte :

```
B f1a_last_gf              <- pointeur du DERNIER graphe soumis
B f1a_last_gf_nodes        <- son nombre de noeuds
T f1a_sub_node_op(const void *)
T f1a_sub_node_src(const void *, int)
T f1a_ptr_in_submitted_graph(const void *)
T f1a_submitted_graph_output_count()
T f1a_eligible(const ggml_tensor *)
```

`f1a_last_gf` est déclaré `const void * f1a_last_gf = nullptr;`
(`f1a-capture.cpp:91`), accompagné de `int f1a_last_gf_nodes = 0;` (`:92`).

Et ggml expose une **API publique d'énumération** (`ggml/include/ggml.h`) :

```
GGML_API struct ggml_tensor * ggml_graph_node   (struct ggml_cgraph * cgraph, int i);  // :2783
GGML_API int                  ggml_graph_n_nodes(struct ggml_cgraph * cgraph);          // :2785
```

`ggml_tensor` est **public** et porte les champs nécessaires :
`enum ggml_type type` (`:684`), `int64_t ne[GGML_MAX_DIMS]` (`:688`),
`size_t nb[]` (`:691`), **`enum ggml_op op`** (`:695`), plus `src[]` et `name`.

**Aucune modification de Q1 n'est donc requise.** Tous les symboles sont
exportés, toutes les structures et fonctions sont publiques.

---

## 3. Niveau observé — établi

`OBSERVED_GRAPH_LEVEL = GRAPHE SOUMIS (submitted graph)`

Preuve de niveau : la colonne `gf` de `SUBMITTED_GRAPH_SNAPSHOTS.csv` vaut
`0xfbdeec9f3030` dans le run Q1 réel — exactement le pointeur que
`f1a_last_gf` publie. La surface et l'accès désignent **le même objet**.

C'est le niveau que la question NextN exige : *le graphe fourni à la
soumission*. Ni le graphe construit avant transformation, ni un graphe
reconstruit dans un contexte parallèle — ce que §4 interdit.

---

## 4. Pourquoi l'accès 1 (`cb_eval`) n'est pas retenu

`cb_eval` existe bien, mais §4 pose deux limites qui s'appliquent :

- **« Ne pas supposer qu'un callback par tenseur donne accès au graphe complet. »**
  `cb_eval` est appelé **par tenseur** pendant le calcul ; il ne fournit pas
  l'énumération du graphe soumis.
- Dans llama.cpp, `cb_eval` force une synchronisation par nœud et **désactive
  les CUDA graphs**. §4 interdit explicitement de modifier le chemin
  d'exécution « pour obtenir un chemin plus facile à observer ».

L'accès 1 reste disponible comme **contrôle croisé** ultérieur, mais il ne peut
pas être la source principale, car il changerait le chemin observé.

---

## 5. Effets de l'observateur retenu

- **Sur le calcul : aucun.** Lire `f1a_last_gf`, appeler `ggml_graph_n_nodes` et
  `ggml_graph_node`, lire les champs d'un `ggml_tensor` : ce sont des lectures
  de structures déjà construites.
- **Sur l'exécution : aucun.** Aucune synchronisation ajoutée, aucun CUDA graph
  désactivé, aucun paramètre scientifique touché.
- **Condition d'existence :** `f1a_last_gf` est renseigné **au moment de la
  soumission**. Il faut donc que le chemin de soumission soit passé — donc que
  `OBRAIN_F1A_CAPTURE=1` ait été actif, ou à défaut que `f1a_last_gf` ait été
  positionné par la soumission elle-même. **Ce point doit être constaté au
  premier run** : lire `f1a_last_gf` sans soumission donne `nullptr`.
- **Risque de mutation :** `f1a_last_gf` désigne le **dernier** graphe soumis.
  Une seconde soumission écrase le pointeur. L'énumération doit donc être faite
  **dans la fenêtre** entre la soumission et la suivante, et corrélée par
  `submission_id`.

---

## 6. Ce qui reste non établi

- **La validation sur un forward réel.** Aucun forward n'est autorisé ici :
  `RUNTIME_OBSERVER_VALIDATION=NOT_EXERCISED`.
- **Le dictionnaire de correspondance `ggml_op` → nom.** `ggml_op_name` doit
  être confirmé comme exporté avant d'être utilisé comme dictionnaire.
- **La stabilité du pointeur.** Que `f1a_last_gf` désigne bien le graphe de la
  soumission courante — et non un résidu — doit être constaté au premier run
  en le confrontant à la colonne `gf` de `SUBMITTED_GRAPH_SNAPSHOTS.csv`.
