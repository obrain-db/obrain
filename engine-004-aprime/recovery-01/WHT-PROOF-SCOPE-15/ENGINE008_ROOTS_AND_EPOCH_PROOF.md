# ENGINE008_ROOTS_AND_EPOCH_PROOF (WHT-PROOF-SCOPE-15)

## 1. La racine des logits, chaine source exacte

`src/models/qwen35.cpp`, constructeur STANDARD `llama_model_qwen35::graph` :

```cpp
:225    cb(cur, "h_nextn", -1);
:226    res->t_h_nextn = cur;                    // etat du tronc PUBLIE
:228    if (!cparams.embeddings_nextn_masked && inp_out_ids) {
:229        cur = ggml_get_rows(ctx0, cur, inp_out_ids);   // <-- cur REASSIGNE
:231    cb(cur, "result_norm", -1);
:239    res->t_logits = cur;                     // <-- LA SORTIE LOGITS
:241    ggml_build_forward_expand(gf, cur);      // <-- LA RACINE DES LOGITS
```

**`QWEN35_LOGITS_ROOT_IDENTIFIED=YES`.** La racine des logits est le tenseur
`cur` **apres** `output_norm` et **apres** la selection `ggml_get_rows(..., inp_out_ids)`.

La cloture de cette racine contient le dernier etat cache du tronc, donc **toutes
les projections du tronc** -- et par consequent les WHT inserees par
`build_lora_mm` sur les poids du tronc.

## 2. Les racines AUXILIAIRES existent, et sont nombreuses

`ggml_build_forward_expand` est appele ailleurs dans `llama-graph.cpp`
(`:2112 weights`, `:2266 experts`, `:2276 cur_experts[i]`, `:2288 moe_out`,
`:2386 cur`, `:2716 cur`, `:2763-2765 q_cur/k_cur/v_cur`, `:2887-2889
q_cur/v_cur/k_cur`).

`q_cur`, `k_cur`, `v_cur` sont des racines **auxiliaires** (mise en cache KV) :
elles ajoutent des tenseurs au graphe **sans** etre sur le chemin des logits.
Cela confirme la nuance de A3 : **appartenir au graphe n'implique pas atteindre
les logits.**

## 3. Ce que les champs producteur etablissent REELLEMENT

```cpp
// f1a-capture.cpp:708
const int down_ok = f1a_node_reaches_declared_output(r.t_mm_capture) ? 1 : 0;
// f1a-capture.h:73
bool f1a_node_reaches_declared_output(const void * node);
```

`downstream_reach=1` signifie : le noeud atteint **UN** output declare -- c'est-a-dire
un tenseur marque `GGML_TENSOR_FLAG_OUTPUT` (`ggml_set_output`). Or le
constructeur declare **plusieurs** sorties : `t_logits`, `t_embd` (`:233`),
`t_h_nextn` (`:226`).

**`ROLE_EFFECT_FIELD_SEMANTICS_ESTABLISHED=YES`**, et la limite est conservee :
**le champ ne distingue PAS les racines.** « un libelle OUTPUT_DEPENDENT ne
suffit pas » -- exactement. Il etablit « atteint une sortie declaree », **pas**
« atteint la sortie des logits ».

**La distinction est donc faite par la SOURCE, pas par le champ** : §1 etablit la
racine des logits ; `get_rows` ne coupe pas le tronc ; les WHT du tronc sont dans
sa cloture.

## 4. Les cibles capturees sont-elles des poids du TRONC ? -- verifie

Mesure sur les 672 lignes du manifeste :

| Grandeur | Valeur |
|---|---|
| noms de poids distincts | **168** |
| couches concernees | **blk.8 a blk.55** (48 couches) |
| noms hors `blk.` | **aucun** |
| couches `>= 64` (donc NextN) | **AUCUNE** |

**Toutes les cibles capturees sont des poids du TRONC**, aucune n'est un poids
NextN. Elles sont donc toutes dans la cloture de la racine des logits.

`WHT_TO_LOGITS_MAPPING=COMPLETE` pour les cibles observees -- etabli par la
source (§1) et corrobore par l'inventaire ci-dessus.

## 5. Portee de CAPTURE_EXTRACT et contrat d'epoch

```cpp
// f1a-capture.cpp
:564   if (cc->targets.empty()) { cc->epoch++; return; }
:586   cc->targets.clear(); cc->act_key_to_id.clear(); cc->epoch++;
:601   const unsigned long long epoch_capture_id = cc->epoch;
:671   FILE * rm = fopen((g_f1a.outdir + "/GRAPH_ROLE_RELATIONSHIPS.csv").c_str(), "w");
:675   FILE * fp = fopen((g_f1a.outdir + "/TARGET_EFFECT_PATH.csv").c_str(), "w");
:678   FILE * be = fopen((g_f1a.outdir + "/BACKEND_OWNERSHIP_MANIFEST.csv").c_str(), "w");
:826   f1a_dump_submitted_snapshots();
:839   cc->epoch++;
```

**LES TROIS SURFACES ROLE/EFFET/RESIDENCE SONT OUVERTES EN `"w"` : ECRASEMENT
INTEGRAL A CHAQUE EXTRACTION.**

C'est la reponse exacte a la distinction demandee :

| Cas | Verdict |
|---|---|
| analyse volontairement limitee a l'epoch courant | **NON** |
| **analyses anterieures produites puis ECRASEES** | **OUI** -- `fopen("w")` |
| analyse jamais produite pour le prefill | **NON** -- elle EST produite, puis ecrasee |

**`CAPTURE_EXTRACT_EPOCH_SCOPE_ESTABLISHED=YES`.**

### Les 168 associations manquantes, expliquees depuis cette chaine

Mesure : `kinds par epoch` -> `epoch 0` : 168 `activation` + 168 `base_output` ;
`epoch 1` : idem. Les DEUX epochs portent les memes 168 `target_id` (le compteur
de cibles est **remis a zero** a chaque extraction, `cc->targets.clear()` :586).

Les fichiers roles/effet/residence etant ecrases a chaque extraction, **seul le
dernier epoch survit** (epoch 1, le decode). L'analyse de l'**epoch 0 (le
prefill)** a bien ete **produite, puis ecrasee**.

**=> Les 168 manquantes sont les cibles du prefill, dont l'analyse a ete
detruite par l'ecriture en `"w"`.** Ce n'est ni une limite volontaire, ni une
absence de production.

## 6. Consequence pour le futur preflight -- et pourquoi la collecte par fenetre compte

Un run reel a **1 prefill + N pas de decode** = **N+1 extractions**, chacune
ecrasant la precedente. **Lire les fichiers une seule fois a la fin ne
donnerait que la derniere.**

**C'est pourquoi le harnais collecte le WHT DANS CHAQUE FENETRE** (ajoute en
PROOF-13 : `wht_collect` appele apres le prefill et apres chaque pas de decode).
En lisant avant l'ecrasement suivant, la collecte par fenetre reconstitue ce que
le producteur ne conserve pas.

**Aucune modification de Q1 n'est necessaire** : la meme methode analyse chaque
fenetre, a condition que chaque extraction soit terminee dans sa fenetre. C'est
precisement ce que la confirmation runtime doit etablir.
