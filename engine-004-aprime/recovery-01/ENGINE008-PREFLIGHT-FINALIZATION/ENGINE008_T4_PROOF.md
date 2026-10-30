# T4 -- LES QUATRE CAS, FERMES

Source de reference : `src/llama-graph.cpp:1480-1540` (`build_lora_mm`), lue verbatim.

## Le point decisif, verbatim

```cpp
    // [F1A-CAPTURE] capture diagnostique : inerte sans OBRAIN_F1A_CAPTURE=1.
    ggml_tensor * res;
    if (f1a_cc != nullptr && f1a_eligible(w)) {
        ggml_tensor * const f1a_cur_cap = ggml_dup(ctx0, cur);            // DUP de l ACTIVATION
        f1a_mark_output(f1a_cur_cap);
        ggml_tensor * const f1a_mm_raw  = ggml_mul_mat(ctx0, w, f1a_cur_cap);  // LE matmul REEL
        res = ggml_dup(ctx0, f1a_mm_raw);                                 // DUP du RESULTAT
        f1a_mark_output(res);
        f1a_capture_note(f1a_cc, w, f1a_cur_raw, cur, f1a_cur_cap, f1a_mm_raw, res);
    } else {
        res = ggml_mul_mat(ctx0, w, cur);                                 // chemin NORMAL
    }
```

**Dans les DEUX branches, il y a exactement UN `ggml_mul_mat(ctx0, w, ...)`.** La
branche de capture ajoute deux `ggml_dup` dont le `src0` est l'**activation**
(`f1a_cur_cap`) ou le **resultat du matmul** (`f1a_mm_raw`) -- **jamais le poids**.

## 1. COPIE DE CAPTURE -- `JUSTIFIED`

Les copies sont des `GGML_OP_DUP`. Notre filtre retient `GGML_OP_MUL_MAT` et
`GGML_OP_MUL_MAT_ID` **et** exige `src0 == poids`. Les `dup` sont donc **exclues
structurellement**, par leur opcode ET par leur `src0`.

Concordance avec le producteur : `f1a_capture_note` fait lui-meme
`f1a_ctr_dup_added += 2` en commentant « R4-a : cur_capture + mm_capture ». **Le
producteur les compte comme des DUP, pas comme des cibles.**

Teste par le chemin raccorde (`t-t4`) : 3 invocations + 3 ops NON-matmul portant
le poids -> `n_expected_invocations=3`, `scan_nonmatmul=3`, verdict COMPLETE.
Une copie ne devient donc **pas** une invocation supplementaire.

## 2. ALIAS / VUE DU POIDS -- `JUSTIFIED`

`build_lora_mm(w, cur, w_s)` passe `w` **directement** a `ggml_mul_mat`. Aucune
vue, aucun `ggml_cont`, aucun reshape n'est cree sur `w` dans ce chemin : le
`src0` du matmul **EST** le tenseur enregistre.

**Et si un `src0` n'etait pas resolu au registre** (alias, vue, tenseur
intermediaire), il n'est **pas attribue arbitrairement** : il est **COMPTE**
(`scan_unregistered_src0`), une limite est publiee
(`MATMUL_SRC0_NOT_IN_REGISTRY`), et la **completude est INTERDITE** avec une
reference ambigue nommee. Teste : `unregistered_src0_blocks_completeness`.

**Aucune attribution depuis un nom ou un pointeur voisin.**

## 3. EXPERT NON SELECTIONNE -- `JUSTIFIED`

Quatre notions DISTINGUEES :
1. **poids charge** -- dans le registre natif ;
2. **operation construite** -- un noeud cree dans le graphe ;
3. **operation presente dans le graphe SOUMIS** -- ce que nous enumerons ;
4. **selection effectuee pendant le calcul** -- hors de notre portee.

Nous comptons **(3)**. Un expert **charge mais non selectionne** n'a **pas** de
noeud dans le graphe soumis -> il n'est **pas** compte. Teste :
`loaded_but_unselected_weight_not_counted` : 3 poids eligibles, 2 operations ->
`n_eligible_weights=3`, `n_expected_invocations=2`. **Le nombre d'experts charges
n'est PAS assimile au nombre de cibles attendues.**

`GGML_OP_MUL_MAT_ID` (chemin MoE) **est compte** : teste
(`mul_mat_id_path_would_be_counted`). Pour le perimetre Q1 examine, le manifeste
ne porte **aucun** suffixe `_exps` ni `shexp` -- les suffixes presents sont
`attn_gate, attn_k, attn_output, attn_q, ffn_gate, ffn_up`.

## 4. AUTRE OPERATION UTILISANT LE MEME POIDS -- `JUSTIFIED`

L'opcode ET l'identite du poids suffisent **parce que la correspondance est
demontree sur le chemin applicable** : `build_lora_mm` ne cree que
`ggml_mul_mat`, et les seules autres operations qu'il ajoute sont des `ggml_dup`
et un `ggml_mul(ctx0, res, w_s)` dont le `src0` est `res`, **pas le poids**.
Toute autre operation d'un autre helper n'est pas sur ce chemin.

Une operation NON matmul portant le poids est **comptee separement**
(`n_nonmatmul_with_weight`) et **jamais** retenue comme invocation. Teste :
`shared_weight_nonmatmul_NOT_an_invocation` (5 ops non-matmul + 3 matmul ->
`n_expected_invocations=3`, `scan_nonmatmul=5`).

## Fixtures : structures reelles, pas des etiquettes

Les tests ne posent pas d'etiquette dans un nom : ils construisent des
`MatmulScan` avec les **comptes reels** que l'adaptateur produit (noeuds,
matmul avec poids, src0 non resolu, ops non-matmul) et verifient le **rapport
serialise**. Et `t-e2e` traverse l'inventaire lu au format reel, un registre
mocke et un graphe synthetique **de bout en bout par `run_pipeline`**.
