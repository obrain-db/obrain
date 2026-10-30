# ENGINE008_GRAPH_ENUMERATION_SCHEMA

**Schéma proposé, versionné : `ENGINE008_GRAPH_ENUM_v1`**
**Statut : SPÉCIFIÉ, NON IMPLÉMENTÉ dans cette passe.**

## 1. Enregistrement par nœud

Une ligne par nœud, ordre = ordre du graphe soumis (`ggml_graph_node(g, i)`).

```
schema,run_id,submission_id,graph_ptr,graph_fingerprint,n_declared,n_exported,
node_index,op_code,op_name_available,tensor_name_or_empty,tensor_name_len,
type_code,n_dims,ne_0,ne_1,ne_2,ne_3,is_leaf,is_output_flag,
n_src,src0_index,src1_index,src2_index,src3_index,
has_nextn_leaf,name_is_empty
```

Notes de conception :

- `op_code` est **normatif** ; `op_name_available` signale seulement si le
  dictionnaire a pu être résolu. Ne jamais classer sur le nom.
- `n_declared` vient de `ggml_graph_n_nodes` ; `n_exported` est le nombre de
  lignes réellement écrites. **`n_declared != n_exported` ⇒ capture incomplète.**
- `srcK_index` référence un **index local** de nœud ou de feuille, jamais une
  adresse. Une adresse est un moyen de corrélation **local**, pas une identité
  structurelle portable.
- `has_nextn_leaf` et `name_is_empty` sont des **indicateurs d'ambiguïté**, pas
  des conclusions.
- `graph_ptr` est conservé pour corréler avec la colonne `gf` du snapshot f1a ;
  `graph_fingerprint` pour corréler avec la colonne `fingerprint`.

## 2. Champs délibérément EXCLUS

| Champ | Motif |
|---|---|
| données numériques des tenseurs | aucune évaluation numérique n'est autorisée ni nécessaire |
| `data`, `buffer`, `view_src` (adresses internes) | corrélation locale ; non portables |
| digest des nœuds | §6 : ne pas remplacer les données énumérées par leur hash |
| champs non observables | §6 : ne pas déclarer un champ renseigné quand l'accès ne le permet pas |

## 3. Contrôles d'exhaustivité exigés

Une capture n'est **COMPLETE** que si, cumulativement :

1. `f1a_last_gf != nullptr` (une soumission a bien eu lieu) ;
2. `n_exported == n_declared` ;
3. chaque `srcK_index` non nul résout vers un nœud ou une feuille **présente** dans l'export ;
4. `graph_ptr` **égale** la colonne `gf` du `SUBMITTED_GRAPH_SNAPSHOTS.csv` pour le même `submission_id` ;
5. `graph_fingerprint` **égale** la colonne `fingerprint` du même enregistrement ;
6. `submission_rc == 0` pour cette soumission.

**Un fichier non vide ne suffit jamais.** Toute condition non satisfaite produit
un état `NOT_CONFIRMED_*`, jamais une conclusion.

## 4. Règles de classification NextN (conservatrices, préenregistrées)

| Résultat | Condition |
|---|---|
| `NEXTN_STRUCTURE_DETECTED` | une **opération** du graphe soumis consomme, directement ou via une chaîne de dépendance, une feuille NextN |
| `NEXTN_NOT_DETECTED_IN_COMPLETE_SUBMITTED_GRAPH_SCOPE` | capture COMPLETE **et** aucune opération de ce type — porte sur le **périmètre des graphes soumis** uniquement |
| `NOT_CONFIRMED_INCOMPLETE_COVERAGE` | l'un des six contrôles du §3 échoue |
| `NOT_CONFIRMED_AMBIGUOUS_MAPPING` | `graph_ptr` ou `fingerprint` ne corrèle pas, ou un `srcK` ne résout pas |
| `MALFORMED_CAPTURE` | schéma non respecté |
| `SURFACE_INSUFFICIENT` | aucun accès disponible (n'est plus le cas) |

Le deuxième résultat **ne doit pas** être renommé « absence d'exécution runtime
confirmée ». Il porte sur la construction du graphe, pas sur l'exécution.
