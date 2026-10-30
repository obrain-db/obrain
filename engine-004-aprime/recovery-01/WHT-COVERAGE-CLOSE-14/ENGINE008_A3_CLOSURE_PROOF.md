# ENGINE008_A3_CLOSURE_PROOF — portee de `ggml_build_forward_expand`

## Le code, verbatim (`ggml/src/ggml.c:7104`)

```cpp
void ggml_build_forward_expand(struct ggml_cgraph * cgraph, struct ggml_tensor * tensor) {
    ggml_build_forward_impl(cgraph, tensor, true, true);
}

static void ggml_build_forward_impl(struct ggml_cgraph * cgraph, struct ggml_tensor * tensor,
                                    bool expand, bool compute) {
    const int n_old = cgraph->n_nodes;
    ggml_visit_parents_graph(cgraph, tensor, compute);   // parcourt les src[] recursivement
    const int n_new = cgraph->n_nodes - n_old;
    if (n_new > 0) {
        // the last added node should always be starting point
        GGML_ASSERT(cgraph->nodes[cgraph->n_nodes - 1] == tensor);
    }
}
```

## Ce qui est etabli

| Question | Reponse |
|---|---|
| quelles racines sont ajoutees ? | **la racine passee en argument**, et elle est le **DERNIER noeud** du graphe |
| comment les `src[]` sont parcourus ? | `ggml_visit_parents_graph`, recursion sur les parents |
| feuilles vs noeuds ? | les feuilles (sans `src`) sont enregistrees separement ; seuls les noeuds calculables entrent dans `nodes[]` |
| noeuds deja visites ? | ecartes via `visited_hash_set` -- dedoublonnage, pas de re-ajout |
| dependances hors du graphe ? | **celles qui ne sont pas atteignables depuis une racine ajoutee** |

## LA NUANCE DECISIVE

`ggml_build_forward_expand` ajoute la **cloture transitive d'UNE racine**. Un
graphe construit par **plusieurs appels** contient donc l'**UNION des clotures de
plusieurs racines**.

**Consequence : appartenir au graphe n'implique PAS etre sur le chemin d'une
sortie donnee.** Un noeud issu de la racine A peut n'avoir aucun chemin vers la
racine B. Le code lui-meme distingue les racines : `ggml_build_forward_select`
passe `compute = (i == idx)` et n'active le calcul que pour la racine
selectionnee.

**Donc les quatre proprietes sont DISTINCTES, et je ne les confonds pas :**

| # | Propriete | Etat |
|---|---|---|
| 1 | **appartenance au graphe** | ETABLIE -- `in_submitted_graph=1` (504/504) |
| 2 | **dependance d'UNE sortie** | **PARTIELLE** -- exige de savoir DE QUELLE racine |
| 3 | **dependance des LOGITS** | **PARTIELLE** -- exige que la racine pertinente soit celle des logits |
| 4 | **traitement par le backend** | ETABLIE -- le scheduler itere tous les `n_nodes` (`ggml-backend.cpp:947,1045,...`) |
| 5 | **achevement synchronise** | ETABLIE pour la soumission ; ordre exact vis-a-vis des logits = `NOT_EXERCISED` |

**`GRAPH_DEPENDENCY_CLOSURE_ESTABLISHED=YES`** -- la propriete de cloture est
etablie pour la racine soumise. **`WHT_TO_RELEVANT_OUTPUT_MAPPING=PARTIAL`** --
parce que 2 et 3 exigent la liste des racines du graphe qwen35 soumis, que je
n'ai PAS relevee.

**CORRECTION DE PROOF-13** : j'y avais ecrit que « la WHT est un noeud de ce
graphe et son resultat atteint la sortie » en m'appuyant sur
`downstream_reach=1` et `effect_class=OUTPUT_DEPENDENT` du **producteur**. Ces
champs corroborent, mais **leur semantique n'a pas ete lue dans la source** : je
ne les traite donc plus comme etablissant 2 et 3. La deduction de PROOF-13 est
**affaiblie a PARTIAL** en consequence.

## Hypothese restante

**A3-residuel** : etablir la liste des racines effectivement ajoutees au graphe
soumis par le constructeur qwen35, et identifier laquelle produit les logits.
Sans cela, « la WHT a ete executee » reste une deduction **conditionnelle a
l'appartenance de la WHT au sous-graphe des logits**.
