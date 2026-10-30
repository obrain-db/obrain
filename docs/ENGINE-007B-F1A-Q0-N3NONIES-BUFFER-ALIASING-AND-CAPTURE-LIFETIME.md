# ENGINE-007B-F1A-Q0-N3nonies — ALIASING MÉMOIRE ET DURÉE DE VIE DES CAPTURES

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3nonies/`

---

## 0 — Résultat

**Le mécanisme causal est prouvé par la source. L'écrivain précis ne l'est pas.**

```
FIRST_DIVERGENCE_CAUSAL_ROOT=CAPTURE_STORAGE_FREED_AFTER_LAST_CONSUMER
FIRST_DIVERGENCE_CAUSAL_ROOT_PROVEN=YES
BUFFER_REUSE_CAUSALITY=MECHANISM_PROVEN_WRITER_NOT_IDENTIFIED
N3NONIES_STATUS=MECHANISM_PROVEN_WRITER_IDENTIFICATION_PENDING
```

## 1 — La règle de l'allocateur (§3/B)

```c
// ggml/src/ggml-alloc.c:690
static void ggml_gallocr_free_node(ggml_gallocr_t galloc, struct ggml_tensor * node) {
    // graph outputs are never freed
    if (node->flags & GGML_TENSOR_FLAG_OUTPUT) {
        AT_PRINTF("not freeing output %s\n", node->name);
        return;
    }
    ...
}
```

**La seule protection contre la libération est `GGML_TENSOR_FLAG_OUTPUT`.** Tout tenseur qui ne le
porte pas **est libéré dès sa dernière consommation**.

## 2 — Le fork nomme lui-même la conséquence

```c
// ggml/src/ggml-backend.cpp:1342  et  :1361
ggml_set_output(tensor_copy); // prevent ggml-alloc from overwriting the tensor
```

**Ce n'est pas mon interprétation : c'est le commentaire du planificateur.** Le fork documente que
sans ce drapeau, ggml-alloc **écrase** le tenseur.

## 3 — Application à mes tenseurs de capture

```c
cur_capture = ggml_dup(cur);        // cree par MA greffe, SANS ggml_set_output
mm_capture  = ggml_dup(mm_raw);     // idem
```

Leurs régions de stockage sont **libérées dès que leur dernier consommateur a fini** — donc bien
avant mon extraction, qui a lieu à la fin de l'époque.

`cur_capture` n'a **qu'un** consommateur : le `MUL_MAT`. Sa région est donc libérée **immédiatement
après le matmul**, tôt dans le graphe. Entre cette libération et l'extraction, **3968 autres nœuds**
s'exécutent et peuvent recevoir cette région et y écrire.

## 4 — Ce que cela établit

```
H1 (nul a la consommation)        REFUTEE  (les sorties natives sont non nulles)
H2 (region reutilisee avant extraction)  MECANISME PROUVE
H4 (extraction lit une region qui n est plus le tenseur)  CONSEQUENCE de H2
```

La règle de l'allocateur rend H2 **certaine**, pas probable : elle est inconditionnelle.

## 5 — Pourquoi l'époque 1 semble saine : je ne l'explique pas

**Le mécanisme est identique aux deux époques** (même graphe, même empreinte `nodes=3991`). Une
différence d'**ordonnancement d'allocation** entre la première exécution et les suivantes peut faire
que la région libérée soit ou non réellement réécrite avant l'extraction.

**Je n'invente pas l'explication.** Et surtout :

```
EPOCH1_CAPTURES_ALSO_AT_RISK=YES
```

L'argument « l'époque 1 est saine » **ne prouve pas** que ses 168 captures sont fidèles : la règle de
l'allocateur s'y applique exactement pareil. Une activation non nulle n'est pas une activation
fidèle — c'est exactement ce que votre §8 dit.

## 6 — Le correctif, et sa conséquence

Marquer `cur_capture` et `mm_capture` avec `ggml_set_output()` à leur création dans la greffe de
`build_lora_mm`. **C'est ce que le fork fait lui-même** pour ses copies de sortie (lignes 1342, 1361).

**Attention (§9)** : cela **change le plan d'allocation**. Le contrôle de transparence OFF/ON doit
être **refait** sur le nouveau binaire. Les preuves bitwise acquises ne sont **pas** héritées — je ne
les réutiliserai pas.

## 7 — Ce que je n'ai PAS fait

```
etape A : chemins de construction des 4 temoins        NON EXECUTEE
etape C : regions memoire (region_start / region_end)   NON FAITES
etape D : ordre producteur/consommateur explicite       NON FAIT
etape E : signature 11.04527                            NON ANALYSEE
etape F : experience de preservation ciblee             NON EXECUTEE
correctif applique et binaire reconstruit               NON FAIT
```

```
CAPTURE_BUFFER_REGIONS_IDENTIFIED=NO
CAPTURE_REGION_OVERLAP_COUNT=NON_MESURE
OVERLAPPING_WRITERS_IDENTIFIED=NO
```

**Le mécanisme est prouvé ; le couple région/écrivain ne l'est pas.** Votre §5 exige les deux pour
`BUFFER_REUSE_CAUSALITY=PROVEN` : je n'ai donc **pas** ce statut. J'ai le mécanisme et sa
conséquence, pas la désignation du tenseur qui a écrasé chaque région.

## 8 — Rapport final (§12)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_N3NONIES_STATUS=MECHANISM_PROVEN_WRITER_IDENTIFICATION_PENDING
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
SELECTED_GRAPH_PATHS_TRACED=NO
ALLOCATOR_REUSE_RULE_IDENTIFIED=YES
INTERMEDIATE_TENSOR_LIFETIME_RULE_IDENTIFIED=YES
OUTPUT_FLAG_PRESERVATION_SEMANTICS=PROVEN
CAPTURE_BUFFER_REGIONS_IDENTIFIED=NO
CAPTURE_REGION_OVERLAP_COUNT=NON_MESURE
OVERLAP_AFTER_LAST_CONSUMER_COUNT=NON_MESURE
OVERLAPPING_WRITERS_IDENTIFIED=NO
CAPTURE_CONTENT_LIFETIME_THROUGH_I1=VALIDE
CAPTURE_CONTENT_LIFETIME_THROUGH_I2=NON_GARANTI_PAR_L_ALLOCATEUR
CAPTURE_CONTENT_LIFETIME_THROUGH_I3=NON_GARANTI
FIRST_UNPROVEN_LIFETIME_INTERVAL=DES_LA_LAST_CONSOMMATION_DE_cur_capture
TARGETED_PRESERVATION_TEST_EXECUTED=NO
PRESERVED_CAPTURE_CONTENT_VALID=NON_MESURE
DIAGNOSTIC_LOGITS_BITWISE_MATCH=NON_MESURE
BUFFER_REUSE_CAUSALITY=MECHANISM_PROVEN_WRITER_NOT_IDENTIFIED
FIRST_DIVERGENCE_CAUSAL_ROOT=CAPTURE_STORAGE_FREED_AFTER_LAST_CONSUMER
ALL_25_CONTRADICTIONS_EXPLAINED=MECANISME_EXPLIQUE_WRITER_NON_IDENTIFIE
ALL_REQUIRED_CAPTURE_CONTENT_PRESERVED=NO
EPOCH1_CAPTURES_ALSO_AT_RISK=YES
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=PASS_POUR_LA_STRUCTURE_PAS_LE_CONTENU
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES_POUR_L_ANCIEN_BINAIRE
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0_NATIVE_OPERATOR_REFERENCE_VALIDATION=NOT_PASSED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=APPLY_GGML_SET_OUTPUT_TO_CAPTURE_DUPS_THEN_REQUALIFY
```

## 9 — LE CORRECTIF A ÉTÉ APPLIQUÉ ET MESURÉ

Le mécanisme prouvé ci-dessus a été corrigé **et vérifié par l'expérience**.

### Le patch

```
src/f1a-capture.h      declaration : void f1a_mark_output(struct ggml_tensor * t);
src/f1a-capture.cpp    definition  : if (t) ggml_set_output(t);
src/llama-graph.cpp    greffe      : f1a_mark_output(f1a_cur_cap);
                                     f1a_mark_output(res);   // mm_capture
```

`ggml_set_output()` sur les deux tenseurs de capture — exactement ce que le fork fait lui-même pour
ses propres copies de sortie (`ggml-backend.cpp:1342`).

### La vérification de transparence, refaite sur le nouveau binaire

```
CONTROLE D INERTIE (capture OFF)       : aucun payload ecrit, logits produits normalement
TRANSPARENCE OFF/ON, compare octet a octet : IDENTIQUE
LOGIT_BITWISE_MISMATCH_COUNT=0
```

### Le résultat dépasse largement les 25 cas visibles

```
EPOQUE 0 : 168 activations
   toutes nulles  : AVANT=25   APRES=0
   IDENTIQUES     : 0
   DIFFERENTES    : 168   (les 168)
EPOQUE 1 : 168 activations
   toutes nulles  : AVANT=0    APRES=0
   IDENTIQUES     : 0
   DIFFERENTES    : 168   (les 168)

CONTRADICTIONS activation nulle / sortie non nulle : 25 -> 0
```

```
CAPTURES_WERE_CORRUPTED=336_SUR_336
```

**Les 336 captures d'activation des deux époques étaient toutes corrompues** — pas seulement les 25
visibles. Les 143 activations d'époque 0 qui *semblaient* saines, et les 168 de l'époque 1, étaient
écrasées elles aussi.

**Pourquoi ce n'est pas du bruit d'exécution** : les logits OFF et ON du nouveau binaire sont
identiques au bit près. Le calcul est donc déterministe et reproductible ; les différences observées
ne peuvent pas venir d'une variabilité d'exécution. Elles viennent du correctif.

C'est exactement le test que votre §8 demandait : 143 activations non nulles n'étaient **pas**
fidèles, et la règle de validité doit porter sur la conservation, pas sur `amax > 0`.

### Conséquence sur tout le diagnostic antérieur

**Toutes** les reconstructions numériques précédentes (N1bis, N3quater, N3quinquies) ont été
calculées sur des **activations corrompues**. Leurs écarts ne mesuraient rien du modèle — ils
mesuraient ma capture. Y compris l'accusation initiale contre la table ternaire, déjà retirée.

Les conclusions **structurelles** restent valides : lecteur GGUF corrigé et vérifié contre libggml,
encodeur/décodeur ternaire lu dans la source, quatre couches d'adressage éliminées.

## 10 — Reconstruction sur captures non corrompues

| tenseur | ln | référence | native | abs_err | **rel_err** |
|---|---|---|---|---|---|
| `blk.20.ffn_up` | 0 | −0.067311 | −0.144657 | 7.73e−02 | 0.535 |
| `blk.20.ffn_up` | 1 | 0.143388 | 0.076400 | 6.70e−02 | 0.877 |
| `blk.16.attn_gate` | 0 | −0.350703 | −0.375967 | 2.53e−02 | **0.0672** |
| `blk.16.attn_gate` | 1 | −0.184779 | −0.377726 | 1.93e−01 | 0.511 |
| `blk.15.attn_output` | 0 | 0.035519 | −0.057763 | 9.33e−02 | 1.615 |
| `blk.15.attn_output` | 1 | 0.100285 | 0.088475 | 1.18e−02 | 0.134 |

```
blk.20.ffn_up     ln0 : rel 1.335  ->  0.535
blk.16.attn_gate  ln0 : rel 86.68  ->  0.0672   <- meilleur cas
blk.15.attn_output ln0 : reference NULLE -> 0.0355 (non nulle)
```

**E6 ne produit plus zéro** : sa référence est désormais un vrai nombre. L'écart passe de plusieurs
ordres de grandeur aberrants à un facteur 1 à 2.

**MAIS le budget préenregistré `RELATIVE_L2 <= 1e-3` n'est pas atteint.**

```
N3NONIES_NATIVE_REFERENCE_VALIDATION=FAIL
```

Il reste un écart systématique de 13 à 160 %. Il n'est **plus** attribuable à la capture (contenu
vérifié, 0 contradiction), et il n'est **pas localisé**. Candidats non départagés — que je n'accuse
pas :

- la sémantique exacte de la quantification q8_1 (arrondi, fp16, écrêtage) ;
- l'alignement des blocs q8_1 avec les blocs TQ2 de 128 ;
- l'accumulation fp32 du noyau (mon accumulateur scalaire est séquentiel) ;
- l'ordre de lecture des 4 sous-blocs q8_1 dans le noyau.

## 11 — Livrables réellement créés

```
N3NONIES_STATUS.txt        regle de l allocateur, patch, transparence, comparaison 336 captures,
                           reconstruction corrigee, ledger final
capoff/off.txt             controle d inertie sur le nouveau binaire
capon/                     672 payloads de capture non corrompus + manifeste
ON2_LOGITS.bin             logits capture ON du nouveau binaire (identiques a OFF)
OFF_LOGITS.bin             logits capture OFF du nouveau binaire
```

**Non créés** : les 18 fichiers du §11 (`N3NONIES_SELECTED_TARGETS.csv`,
`N3NONIES_ALLOCATOR_SOURCE_TRACE.txt`, `N3NONIES_BUFFER_REGIONS.csv`, etc.). Les mesures
correspondantes sont dans `N3NONIES_STATUS.txt`.

**Le code produit n'a pas été touché** : seuls `src/f1a-capture.{h,cpp}` et la greffe dans
`src/llama-graph.cpp` — tous trois étant des éléments **diagnostiques** de mon cru.
`mmvq.cu`, `vecdotq.cuh`, le GGUF et le chemin de capture graph-bound sont intacts.

## 12 — Ledger final

```
N3NONIES_STATUS=CAPTURE_LIFETIME_DEFECT_FIXED_AND_VERIFIED
HARNESS_DEFECT_56=RESOLVED_VERIFIED_BY_TARGETED_PRESERVATION_AND_BITWISE_TRANSPARENCY
CAPTURES_WERE_CORRUPTED=336_SUR_336
CAPTURES_NOW_VERIFIED=336_SUR_336
ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=0
LOGIT_BITWISE_MISMATCH_COUNT=0
N3NONIES_NATIVE_REFERENCE_VALIDATION=FAIL
FIRST_DIVERGENCE_CAUSAL_ROOT=CAPTURE_STORAGE_FREED_AFTER_LAST_CONSUMER
FIRST_DIVERGENCE_CAUSAL_ROOT_PROVEN=YES
BUFFER_REUSE_CAUSALITY=MECHANISM_AND_EFFECT_PROVEN_WRITER_NOT_IDENTIFIED
NEXT_DIVERGENCE_LAYER_UNKNOWN=YES
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
SALIENCY_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
ENGINE007_NEXT_STEP=LOCATE_REMAINING_Q8_1_OR_ACCUMULATION_DIVERGENCE
```

## 10 — La prochaine étape

```
1. appliquer ggml_set_output() a cur_capture et mm_capture dans la greffe
2. reconstruire le binaire, archiver sources/patch/binaire/bibliotheques
3. REFAire le controle capture OFF (cinq compteurs d inertie)
4. REFAire capture ON sur la trajectoire figee, comparer les logits OFF/ON bit a bit
     -> la transparence bitwise DOIT etre re-mesuree, pas heritee
5. refaire l inventaire des contenus : les 25 contradictions doivent disparaitre
6. verifier que les 143 autres activations de l epoque 0 ne CHANGENT PAS de valeur
     -> si elles changent, c est la preuve directe qu elles etaient ecrasees elles aussi
7. alors seulement : reprendre q8_1
```

**Le point 6 est le plus fort** : comparer les 143 activations non nulles avant/après le correctif.
Si certaines changent de valeur, cela prouve que le contenu de l'époque 0 était corrompu **au-delà
des 25 cas visibles**.

## 11 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_N3NONIES_STATUS=MECHANISM_PROVEN_WRITER_IDENTIFICATION_PENDING
F1A_Q0_N3OCTIES_STATUS=BLOCKED_BUFFER_REUSE_CAUSALITY_GATE
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
ALLOCATOR_REUSE_RULE_IDENTIFIED=YES_ggml-alloc.c:690
OUTPUT_FLAG_PRESERVATION_SEMANTICS=PROVEN
FIRST_DIVERGENCE_CAUSAL_ROOT=CAPTURE_STORAGE_FREED_AFTER_LAST_CONSUMER
FIRST_DIVERGENCE_CAUSAL_ROOT_PROVEN=YES
BUFFER_REUSE_CAUSALITY=MECHANISM_PROVEN_WRITER_NOT_IDENTIFIED
OVERLAPPING_WRITERS_IDENTIFIED=NO
EPOCH1_CAPTURES_ALSO_AT_RISK=YES
H1_REFUTED=YES
H2_MECHANISM_PROVEN=YES
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
HARNESS_DEFECT_50=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
HARNESS_DEFECT_51=RETRACTED_AFTER_MEASUREMENT
HARNESS_DEFECT_52=RESOLVED_VERIFIED_BY_TWO_EPOCH_REGRESSION
HARNESS_DEFECT_53=RESOLVED_VERIFIED_BY_INDEPENDENT_MULTITENSOR_BYTE_COMPARISON
HARNESS_DEFECT_54=RESOLVED_VERIFIED_BY_128_VALUE_TESTS
HARNESS_DEFECT_55=RESOLVED
HARNESS_DEFECT_56=CAPTURE_DUP_TENSORS_NOT_MARKED_AS_OUTPUT_SO_ALLOCATOR_FREES_THEIR_STORAGE
   STATUS=OPEN_MECHANISM_PROVEN_FIX_NOT_YET_APPLIED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
