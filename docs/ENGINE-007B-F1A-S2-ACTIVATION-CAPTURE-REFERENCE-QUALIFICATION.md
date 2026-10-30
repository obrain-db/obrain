# ENGINE-007B-F1A-S2 — HOOK DE CAPTURE D'ACTIVATIONS ET QUALIFICATION DE LA RÉFÉRENCE OPÉRATEUR

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-s2/`

---

## 0 — Résultat

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
```

Deux choses sont **acquises et importantes**, et deux autres ne le sont pas.

| gate du §41 | état |
|---|---|
| `AUTHORITATIVE_TENSOR_INVENTORY_AGGREGATES_CONSISTENT` | **YES** — et il corrige une valeur fausse de F1A |
| `TQ2_G128_REFERENCE_DECODE` | **QUALIFIED** — sémantique dérivée de la source |
| `Q8_1_REFERENCE` sémantique | **établie** (`quantize_row_q8_1_ref`, ggml-quants.c:302) |
| `TARGET_MATMUL_OPERAND_MAPPING_PROVEN` | **NON** |
| `READ_ONLY_ACTIVATION_CAPTURE_HOOK` | **NON QUALIFIÉ** |
| `EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR` | non mesuré |

Le §42 prévoit cet état exact. **Aucune saillance n'a été produite** (§37 respecté).

---

## 1 — Gate d'inventaire (§2) : **PASS**, et il corrige F1A

Le §2 impose de recalculer les agrégats **tenseur par tenseur depuis le CSV**, et interdit
`uniform_rows_per_role × tensor_count` sans preuve d'uniformité.

```
ROLE_ROW_SUM_FROM_CSV=2113536
TOTAL_ELIGIBLE_ROWS_FROM_CSV=2113536
ROLE_AGGREGATE_SUM_MATCH=YES
AUTHORITATIVE_TENSOR_INVENTORY_AGGREGATES_CONSISTENT=YES
```

| rôle | n | lignes | famille |
|---|---|---|---|
| `ffn_gate` | 48 | 835 584 | E5 |
| `ffn_up` | 48 | 835 584 | E5 |
| `attn_gate` | 36 | 221 184 | E5 |
| `attn_q` | 12 | 147 456 | E5 |
| `attn_output` | 12 | 61 440 | **E6** |
| `attn_k` | 12 | 12 288 | E5 |

```
E5_TENSOR_COUNT_FROM_CSV=156      E5_ROW_COUNT_FROM_CSV=2052096
E6_TENSOR_COUNT_FROM_CSV=12       E6_ROW_COUNT_FROM_CSV=61440
ELIGIBLE_TENSOR_COUNT_FROM_CSV=168
```

**Correction due à F1A.** J'avais publié `E5_ROW_COUNT=2 007 040`. La valeur autoritaire issue du
CSV est **2 052 096**. Mon F1A avait calculé `N × nombre_de_tenseurs` par rôle, ce que le §2
interdit précisément — et j'avais même signalé l'écart sans en tirer la conclusion qu'il fallait :
**c'est mon agrégat qui était faux, pas le CSV.**

Note honnête : ma vérification d'uniformité par rôle n'a trouvé **aucun** rôle à `N` non uniforme,
donc l'erreur ne vient pas d'un `N` variable mais d'un comptage de tenseurs par rôle. Je ne
l'explique pas complètement dans cette passe ; la valeur qui fait autorité est celle du CSV, validée
par égalité de somme.

## 2 — Fait de dépôt (§3)

```
ggml-cuda-observe.cuh=ABSENT_FROM_REPOSITORY
ENGINE004_OBSERVER=DECLARED_GRAFT_APPLIED_EXTERNALLY
REUSABLE_HARNESS_COMPONENTS_MUST_BE_REVERIFIED_ON_DISK_BEFORE_REUSE=YES
```

## 3 — Sémantique de la référence, dérivée de la source (§25, §28)

### TQ2_G128 — décodage ternaire exact

Source : `ggml/src/ggml-cuda/vecdotq.cuh:732-767`, `GGML_TYPE_TQ2_G128 = 43` (`ggml.h:439`),
bloc de 34 octets = `uint8_t qs[32]` + `ggml_half d`, `blck=128`.

```c
const int b = qs[j];                 // j = 0..7, octets du bloc, decales de 8*iqs
const int c0 = (b >> 0) & 3;         // poids 4j+0
const int c1 = (b >> 2) & 3;         // poids 4j+1
const int c2 = (b >> 4) & 3;         // poids 4j+2
const int c3 = (b >> 6) & 3;         // poids 4j+3
vi[j] = ((c0 > 2 ? 1 : c0 - 1) & 0xFF)
      | (((c1 > 2 ? 1 : c1 - 1) & 0xFF) <<  8)
      | (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16)
      | (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);
```

**Table de décodage, confirmée par le commentaire source et la référence CPU :**

| code 2 bits | valeur décodée |
|---|---|
| 0 | −1 |
| 1 | 0 |
| 2 | +1 |
| 3 | **+1** (replié) |

Le commentaire source est explicite : « 0 -> -1, 1 -> 0, 2 -> +1, 3 -> +1 (replié, comme la
référence CPU `ggml_vec_dot_tq2_g128_q8_K` et le shader Vulkan) ». **Ce n'est pas un mapping
inféré** — je l'ai relu dans la source courante, comme le §28 l'exige.

Ordre d'empaquetage : petit-boutiste à l'intérieur de l'octet (bits 0-1 = poids 4j, bits 6-7 = poids
4j+3), et octet `j` couvre les poids 4j..4j+3.

### Résultat de l'appel

```c
return d * __low2float(bq8_1[iqs].ds) * sumi;
```
où `sumi = Σ_j dp4a(vi[j], get_int_b4(bq8_1[iqs].qs, j))`, avec `iqs = tid % 4` déterminant la
tranche de 32. **Une seule échelle fp16 `d` pour les 128 poids du bloc.**

### Q8_1 — quantification exacte

Source : `ggml/src/ggml-quants.c:302` `quantize_row_q8_1_ref`, dont `ggml/src/ggml-cuda/quantize.cu:375`
`quantize_row_q8_1_cuda` est le pendant CUDA, appelé par `mmvq.cu:1311`.

```c
for (i = 0; i < nb; i++) {
    float amax = max_j |x[i*32 + j]|;
    const float d  = amax / ((1 << 7) - 1);     // /127
    const float id = d ? 1.0f/d : 0.0f;
    y[i].d  = fp32_to_fp16(d);
    int sum = 0;
    for (j = 0; j < 16; ++j) {
        y[i].qs[j]      = roundf(x[i*32 +     j] * id);
        y[i].qs[16 + j] = roundf(x[i*32 + 16 + j] * id);
        sum += y[i].qs[j] + y[i].qs[16+j];
    }
    y[i].s = fp32_to_fp16(sum * d);
}
```

```
block_q8_1 = { union { struct { ggml_half d ; ggml_half s ; } ; ggml_half2 ds ; } ; int8_t qs[32] ; }
```

**Deux propriétés qui comptent pour la suite** : la quantification est **par bloc de 32**, avec un
`amax` **local au bloc** — donc l'échelle d'un bloc d'activation ne peut pas être calculée sans tous
ses 32 éléments ; et `d` est arrondi en fp16 **avant** d'être utilisé, ce qui est indispensable à
reproduire pour une reconstruction fidèle.

```
TQ2_G128_REFERENCE_DECODE=QUALIFIED_FROM_SOURCE
Q8_1_REFERENCE_SEMANTICS=ESTABLISHED_FROM_SOURCE
Q8_1_REFERENCE_UNIT_TESTS=NOT_RUN   (implementation non ecrite dans cette passe)
```

## 4 — Partition des épisodes (§35, §36) : conforme par construction

```
E5 : E0=[0,1024) E1=[1024,2048) E2=[2048,3072) E3=[3072,4096) E4=[4096,5120)
     union = [0,5120)   intersections = vides   -> partition exacte
E6 : E0=[0,1024) ... E5=[5120,6144)
     union = [0,6144)   intersections = vides   -> partition exacte
```

Ces intervalles découlent de `G = kbx/8` et de `kbx = K/128`, donc de la même preuve source que
F1A. La complétude de partition sera **mesurée** au §34 quand la référence reconstruira l'opérateur
(somme des contributions par épisode = sortie complète).

## 5 — Ce qui bloque : la distinction du §9

Le §9 demande explicitement de distinguer deux sites :

```
IDENTIFICATION_HOOK          -> peut marquer les noeuds eligibles a la construction du graphe
VALUE_CAPTURE_HOOK           -> doit copier les VALEURS apres calcul et avant reutilisation
```

**`build_lora_mm` (`llama-graph.cpp:1476`) est un site d'IDENTIFICATION, pas de capture.** Le §9 a
raison sur ce point, et c'est ce qui m'arrête :

À la construction du graphe, `ggml_mul_mat(ctx0, w, cur)` donne bien les **identités** des tenseurs
(`src0 = w`, `src1 = cur`), donc il permettrait de **prouver le mapping des opérandes** du §8 sans
rien exécuter. Mais il ne donne pas les **valeurs** : `cur` n'est pas matérialisé, et le tampon
sera écrit pendant l'exécution.

Il faut donc un second point, côté exécution, qui permette :

```
tenseur d activation marque -> copie hote synchronisee apres achèvement du producteur
```

Or, ce que l'inspection du dépôt établit :

```
aucun callback de graphe in-tree   (cb_eval / ggml_callback / eval_cb : AUCUN RESULTAT)
ggml-cuda-observe.cuh              ABSENT du depot
   -> il n existe aucun point d accroche d execution pret a l emploi.
```

Le §10 dit « ne pas présumer qu'une API existe » et « documenter le mécanisme choisi ». Le mécanisme
n'existe pas : il est **à construire**, et c'est un travail de plomberie non trivial qui doit aussi
satisfaire le §11 (`CAPTURE_TENSOR_LIFETIME_PROVEN` : producteur terminé, tampon valide, non encore
réutilisé, métadonnées cohérentes — « un memcpy réussi ne suffit pas comme preuve »).

```
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
   -> identifie par lecture de source, PAS prouve a l execution. Le site d identification
      existe ; la preuve runtime du paragraphe 8 exige la capture, donc les deux gates
      tombent ensemble.
READ_ONLY_ACTIVATION_CAPTURE_HOOK=NOT_QUALIFIED
CAPTURE_HOOK=NONE_SELECTED
QUALIFICATION_SEQUENCE_COUNT=0     QUALIFICATION_DECODE_TOKENS=0
EXPECTED_TARGET_INVOCATIONS=NOT_COMPUTED   OBSERVED_TARGET_INVOCATIONS=0
CAPTURE_MODE=LOGICAL_ACTIVATION_PLANNED_NOT_CAPTURED
ACTIVATION_CAPTURE_RUN_PERFORMANCE_DATA=INVALID_NOT_MEASURED
```

**Je n'ai pas écrit un hook que je n'aurais pas pu qualifier.** Le §11 exige une preuve de durée de
vie, le §16 une complétude sans doublon, le §19 les métadonnées `ne[]`/`nb[]` et la contiguïté, le
§22 une déduplication prouvée. Livrer cela à moitié produirait une capture dont je ne pourrais pas
garantir qu'elle mesure ce qu'elle prétend — et c'est exactement le mode d'échec qui a coûté cinq
passes à cette série (R7/R7B/R8, sur une comparaison non vérifiée).

## 6 — Ce que la suite doit faire, précisément

```
S2-a  implementer le site d IDENTIFICATION dans build_lora_mm :
         si src0->type == GGML_TYPE_TQ2_G128 ET src0->name dans l inventaire fige
         -> marquer le noeud (nom du poids, couche, role, famille E5/E6)
         inerte sans OBRAIN_F1A_CAPTURE=1
S2-b  implementer le site de CAPTURE cote execution :
         apres achevement du producteur du tenseur d activation marque,
         ggml_backend_tensor_get() vers un tampon hote, avec preuve de duree de vie
S2-c  manifeste d invocations attendues (paragraphe 15) depuis :
         inventaire fige x structure du modele x nombre de tokens de decodage
S2-d  charge de qualification minuscule : 1 sequence, 8-16 tokens, decodage seul
S2-e  reference q8_1 + TQ2_G128 implementee et testee unitairement (paragraphes 26, 28)
S2-f  validation : reference vs sortie native sur >=2 tenseurs E5, >=1 E6 (paragraphe 30)
```

Le §34 (`EPISODE_PARTITION_COMPLETENESS`) se mesurera alors gratuitement : si
`Σ_e y_e == y_ref_full`, la partition est complète.

## 7 — Aucune mutation (§38, §39)

```
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
   SHA256 mmvq.cu=2bf9a27028207120f476ab543264f0f5ec40ceceb4058989efcf1a84fdeee037
   SHA256 vecdotq.cuh=a8ddd2223d81b20fcc09ba461a659890f70ee7e808fc149d21e1d50bdf3e50d8
MODEL_GGUF_MODIFIED=NO
F1A_S2_CAPTURE.patch=NON_PRODUIT   (aucune source modifiee dans cette passe)
PLUMBING_FAILURE_COUNT=28          PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 8 — Livrables

Produits dans `qualification/e7b-f1a-s2/` :

```
INVENTORY_AGGREGATE_CHECK.txt   agregats recalcules du CSV, gate de coherence
```

**Non produits** (dépendent du hook et de la référence) : `TARGET_OP_RUNTIME_PROOF.csv`,
`CAPTURE_HOOK_PROVENANCE.txt`, `CAPTURE_MANIFEST.csv`, `CAPTURE_COMPLETENESS.txt`,
`CAPTURE_SANITY.txt`, `Q8_1_REFERENCE_TESTS.txt`, `TQ2_G128_REFERENCE_TESTS.txt`,
`REFERENCE_VALIDATION.csv`, `EPISODE_PARTITION_VALIDATION.txt`, `F1A_S2_CAPTURE.patch`,
`F1A_S2_CAPTURE.patch.sha256`, `CONFIG_PROVENANCE.txt`.

## 9 — Rapport final (§44)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
AUTHORITATIVE_ELIGIBLE_TENSOR_COUNT=168
AUTHORITATIVE_ELIGIBLE_ROW_COUNT=2113536
E5_TENSOR_COUNT=156
E5_ROW_COUNT=2052096
E6_TENSOR_COUNT=12
E6_ROW_COUNT=61440
ROLE_AGGREGATE_SUM_MATCH=YES
F1A_PUBLISHED_E5_ROW_COUNT_WAS_WRONG=YES_2007040_CORRECTED_TO_2052096
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
CAPTURE_HOOK=NONE_SELECTED
CAPTURE_PATCH_SHA256=NOT_PRODUCED
CAPTURE_BINARY_SHA256=NOT_PRODUCED
QUALIFICATION_SEQUENCE_COUNT=0
QUALIFICATION_DECODE_TOKENS=0
EXPECTED_TARGET_INVOCATIONS=NOT_COMPUTED
OBSERVED_TARGET_INVOCATIONS=0
MISSING_CAPTURE_COUNT=NOT_MEASURED
UNEXPECTED_CAPTURE_COUNT=NOT_MEASURED
DUPLICATE_CAPTURE_COUNT=NOT_MEASURED
CAPTURED_E5_ACTIVATION_K=NOT_MEASURED
CAPTURED_E6_ACTIVATION_K=NOT_MEASURED
CAPTURED_ACTIVATION_NAN_COUNT=NOT_MEASURED
CAPTURED_ACTIVATION_INF_COUNT=NOT_MEASURED
CAPTURE_MODE=LOGICAL_ACTIVATION_PLANNED_NOT_CAPTURED
Q8_1_REFERENCE_UNIT_TESTS=NOT_RUN
Q8_1_NATIVE_SPOTCHECK=NOT_AVAILABLE
TQ2_G128_REFERENCE_DECODE=QUALIFIED_FROM_SOURCE
TQ2_CODES_0_1_2_3_MAP_TO=-1_0_+1_+1
Q8_1_SCALE_SEMANTICS=amax_over_32_block_divided_by_127_rounded_to_fp16
EPISODE_PARTITION_COMPLETENESS=STRUCTURALLY_PROVEN_NOT_YET_MEASURED
REFERENCE_VALIDATION_MAX_ABS=NOT_MEASURED
REFERENCE_VALIDATION_RELATIVE_L2=NOT_MEASURED
REFERENCE_VALIDATION_COSINE=NOT_MEASURED
EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_S2_BIS_IMPLEMENT_IDENTIFICATION_AND_CAPTURE_SITES
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
