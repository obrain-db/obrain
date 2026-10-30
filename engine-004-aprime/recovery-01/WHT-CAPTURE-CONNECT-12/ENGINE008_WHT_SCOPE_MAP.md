# ENGINE008_WHT_SCOPE_MAP — construction / soumission / execution

## 0. DECOUVERTE MAJEURE : le compte 672 n'est pas un compte de cibles

Le manifeste historique porte **672 lignes** mais seulement **336 cibles
distinctes**. Identites mesurees sur pieces
(`.../e7b-f1a-q1-forward-v1/on/GRAPH_CAPTURE_MANIFEST.csv`) :

| Cle | Distincts |
|---|---|
| `(run_id, epoch, target_id, capture_id)` | **336** |
| `(run_id, epoch, target_id, capture_id, kind)` | **672** |
| `(target_id, capture_id, kind)` | 672 |

`kinds` reellement presents : **`activation`** et **`base_output`**. Chaque cible
porte donc **DEUX lignes**. Exemple : `target_id=1177`, `capture_id=109` ->
une ligne `activation` et une ligne `base_output`, toutes deux `rot=1`, `sub=1`.

**Consequences, et elles sont structurantes :**
- `Q1_ROTATION_APPLIED_TRUE_COUNT=672` compte des **lignes**, pas des cibles.
  Le nombre de **cibles distinctes** portant `rotation_applied=1` est **336**.
- `target_id` est **per-epoch**, pas global : l'identite doit inclure `epoch`.
- Toute couverture qui melangerait lignes et cibles **doublerait** le compte.
- L'identite d'une ligne **doit** inclure `kind` ; sans lui, les 672 lignes se
  reduisent a 336 cles et 336 « doublons » apparaissent -- ce qui est exactement
  le faux positif que mes deux premiers tests ont produit.
- Confirme la consigne : « Le compte historique 672 n'est pas automatiquement le
  compte attendu de chaque nouveau run. » Ici il n'est meme pas le compte de
  cibles du run historique.

## 1. Les quatre surfaces, schema et semantique reelle

**`GRAPH_CAPTURE_MANIFEST.csv`** (24 colonnes) -- porte la rotation ET la jointure.
`run_id, epoch, target_id, capture_id, weight_name, family, expected_K,
`**`rotation_applied`**`, kind, path, dtype, ne0..ne3, nb0..nb3, logical_elems,
bytes, contiguous, decode_step, `**`compute_submission_id`**
-> `compute_submission_id` + `decode_step` relient la cible a la **soumission**,
sans jointure lexicale. `path` pointe le fichier de capture.

**`GRAPH_ROLE_RELATIONSHIPS.csv`** (14 colonnes) -- construit -> soumis :
`sub_at_build` (le tenseur etait-il dans le graphe **soumis** a la construction),
`cur_op`, `cur_src0_is_effective`, `mulmat_op`, `mulmat_src0_is_weight`,
`mulmat_src1_is_cur`, `mmcap_op`, `mmcap_src0_is_mmraw`, `downstream_reach`,
`role_ok`, `effect_class` (valeur observee : `OUTPUT_DEPENDENT`).

**`TARGET_EFFECT_PATH.csv`** (9 colonnes) :
`role` (observe : `MM_CAPTURE`), `effect_class`, `src0_is_weight`, `src1_is_cur`,
`dst_is_cur`, `effect_ok`.

**`RESIDENCY_MANIFEST.csv`** (15 colonnes) :
`role` (observe : `CUR_CAPTURE`), `tensor_ptr`, `tensor_name`, `op`, `type`, `ne0`,
`nb0`, `nbytes`, `contiguous`, `buffer_ptr`, `resident`,
`in_submitted_graph`, `weight_name`.
`in_submitted_graph` est le champ d'**appartenance au graphe soumis** ;
`resident`/`buffer_ptr` decrivent la **residence memoire**, pas l'execution.

**Aucune de ces quatre surfaces n'observe l'execution.** Ni `effect_ok`, ni
`role_ok`, ni `in_submitted_graph`, ni `resident` ne portent une trace
d'execution -- malgre leurs noms.

## 2. Portee exacte, maillon par maillon

| # | Maillon | Champ observe | Ce que le champ etablit |
|---|---|---|---|
| 1 | activation entrante | `path` (kind=`activation`) + `cur_raw_id` | l'activation TELLE QU'ELLE ARRIVE |
| 2 | activation consommee | `rotation_applied=1` | la WHT a ete INSEREE ; l'activation consommee est transformee. Cache `obrain_rot` : une activation partagee est servie par le cache, `rotation_applied` reste vrai a juste titre |
| 3 | cible -> operations | `mulmat_src1_is_cur`, `mmcap_src0_is_mmraw`, `role_ok`, `effect_ok`, `effect_class` | la cible alimente bien les operations visees |
| 4 | appartenance au graphe SOUMIS | `sub_at_build`, `in_submitted_graph` | le tenseur appartient au graphe **soumis** |
| 5 | statut du calcul | `compute_submission_id`, `decode_step` + ledger | la **soumission** porteuse a eu lieu ; son `rc` vient du ledger |

## 3. Les QUATRE etats distincts, et ou l'on s'arrete

1. **WHT CONSTRUITE** -- etablie par `rotation_applied=1`.
2. **RESULTAT WHT CONSOMME PAR UNE OP DU GRAPHE SOUMIS** -- etablie si
   `in_submitted_graph=1` **et** `role_ok=1` **et** `effect_ok=1`.
3. **SOUMISSION REUSSIE** -- etablie par le ledger (`submission_rc=0`).
4. **EXECUTION** -- **AUCUNE surface auditee ne l'observe.** Le code retourne
   `EXECUTION_NOT_ESTABLISHED` et l'inscrit dans la portee :
   « EXECUTION NON ETABLIE : aucune surface auditee n'observe l'execution, et une
   reussite de soumission n'est PAS une trace par operation. »

**Je ne transforme pas une reussite de soumission en trace directe de chaque
operation.** C'est la limite du mecanisme disponible, et elle est conservee.

## 4. Couverture attendue, definie depuis les regles du producteur

- **Cible attendue** = tout `w` tel que `w->type == GGML_TYPE_TQ2_G128` **et**
  `w->name` present dans l'inventaire CSV (`f1a-capture.cpp:442`) -- et, pour le
  manifeste, tout couple `(epoch, target_id)` distinct.
- **Cible couverte** = couple present avec `rotation_applied` **exploitable**
  (0 ou 1). Une valeur illisible est **comptee** dans `missing_rotation_field`,
  jamais supposee.
- `rotation_applied=1` sur **aucune** cible -> `INCOMPLETE_COVERAGE`, avec la
  portee « aucune rotation OBSERVEE -- jamais une preuve d'absence ».
- `kind` DISTINGUE `activation` et `base_output` : une couverture par cible exige
  les deux, une couverture par ligne les compte separement.

## 5. Resultat sur la cible candidate

Le manifeste historique, lu par le nouveau parseur : `VALID`, 672 lignes,
672 identites, **336 cibles**, `rotation_applied` T=672 / F=0,
`missing_rotation_field=0`, aucun doublon sur l'identite complete.
`CANDIDATE_P3_ASSESSABLE=YES` -- l'obstacle invoque dans FINALIZE-10 n'existait
pas, et le raccordement est desormais en place.
