# Q1_FORWARD_PROTOCOL_v1 — POST-WHT CORRECTNESS QUALIFICATION

Statut : **GELÉ**. Aucune mesure Q1 n'existait avant ce gel.
Protocole gelé le : 2026-09-30. Dépôt : hors git (`NOT_AVAILABLE`).

---

## §0 — Invariant fondamental

```
NEVER_COMPARE_RAW_VS_POST_WHT=YES
```

Q1 est une **qualification de correction POST-WHT**. Ce n'est **pas** une comparaison
raw-vs-post-WHT. Q0 et Q1 vivent dans **deux espaces de représentation différents**.
Aucun verdict Q1 ne repose sur une différence `Q0 raw output` vs `Q1 post-WHT intermediate`.

Le Q0 peut être cité comme contexte descriptif uniquement (§21).

---

## §1 — Objectif et périmètre

**Objectif.** Établir si une reproduction indépendante de l'opérateur natif reproduit la sortie
native du matmul dans l'espace POST-WHT, sous le budget numérique pré-déclaré.

**Périmètre.** La population WHT-éligible, sur la trajectoire gelée, avec la sémantique de
rotation activée.

**Non-objectifs.** Aucun `PRODUCT_GAIN`, aucune saillance, aucune ablation de qualité, aucun
benchmark de performance. Voir §36.

---

## §2 — Autorités gelées

```
Q1_MODEL_PATH=/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf
Q1_MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937

Q1_TRAJECTORY_REUSED_FROM_Q0=YES
Q1_TRAJECTORY_SHA256=dc7b55dc5e87f7fcc50d2b2af15878cad7d6e5361b8a01836b10a248e7a21096
  INPUT_IDS=9906
  OUTPUT_IDS=279,6511,314,9338,369,220,17,1879,323
  DECODE_STEPS=9
NO_RETOKENIZATION=YES
```

L'autorité de trajectoire est `Q0FWD_TOKEN_TRAJECTORY.txt`, dont le SHA est celui du §2 de Q0.
Aucune retokenisation, aucune réutilisation de `r1/tokens-greedy.txt` (256 tokens, 255 pas — ce
n'est pas cette trajectoire).

---

## §3 — Sémantique d'activation du WHT (audit source)

Source : `src/llama-graph.cpp:1497`

```c
if (w->type == GGML_TYPE_TQ2_G128 && getenv("OBRAIN_NO_ROTATION") == nullptr) {
```

```
WHT_ENABLE_CONDITION_SOURCE =  w->type == GGML_TYPE_TQ2_G128 AND getenv("OBRAIN_NO_ROTATION") == nullptr
WHT_DISABLE_CONDITION_SOURCE = w->type != GGML_TYPE_TQ2_G128 OR  getenv("OBRAIN_NO_ROTATION") != nullptr
OBRAIN_NO_ROTATION_ENV_SEMANTICS = PRESENCE_TEST_ONLY
```

**Le test est `== nullptr` : c'est un test de PRÉSENCE, pas de valeur.**

| état | rotation |
|---|---|
| variable absente | **ACTIVÉE** |
| `OBRAIN_NO_ROTATION=1` | désactivée |
| `OBRAIN_NO_ROTATION=0` | **désactivée** |
| `OBRAIN_NO_ROTATION=""` | **désactivée** |

```
Q1_WHT_ENVIRONMENT_CONTRACT:
  OBRAIN_NO_ROTATION=UNSET     (obligatoire : la variable ne doit PAS exister)
  OBRAIN_F1A_CAPTURE=1  (ON)  /  0  (OFF)
  OBRAIN_F1A_INVENTORY=<TQ2_TENSOR_INVENTORY.csv>
  OBRAIN_F1A_OUTDIR=<dossier du run>
  OBRAIN_F1A_RUN_ID=<identifiant du run>
```

**Interdit : `OBRAIN_NO_ROTATION=0`.** Ce n'est PAS équivalent à absent : `0` désactive la
rotation. C'est exactement le piège que le §4 du brief demandait de ne pas supposer.

**Variables distinguées :**
- *Auditées et pertinentes* : `OBRAIN_NO_ROTATION`, `OBRAIN_F1A_CAPTURE`, `OBRAIN_F1A_INVENTORY`,
  `OBRAIN_F1A_OUTDIR`, `OBRAIN_F1A_RUN_ID`.
- *Non pertinentes pour Q1* : aucune autre variable `OBRAIN_*` n'a été trouvée dans le chemin de
  rotation, de quantification, de sélection MMVQ/MMQ, de layout tensoriel ou de capture.
- *Non auditées* : toute variable d'environnement CUDA/ggml générique. Elles doivent être
  identiques entre OFF et ON, et identiques à celles de Q0.

---

## §4 — Sémantique mathématique du WHT (audit source)

Source d'autorité : `ggml/src/ggml-cpu/ops.cpp:1468` `ggml_compute_forward_wht_f32`.
Le noyau CUDA (`ggml/src/ggml-cuda/wht.cu`) déclare explicitement copier cette sémantique.

```
Q1_WHT_TRANSFORM_FAMILY   = Walsh-Hadamard (papillon rapide FWHT, boucle h=1;h<block;h*=2)
Q1_WHT_DIRECTION          = forward ; auto-inverse (aucune variante inverse séparée)
Q1_WHT_NORMALIZATION      = 1/sqrt(block)   [ops.cpp:1489 : const float norm = 1.0f/sqrtf((float) block)]
                            -> orthonormale, PAS 1/N, PAS 1/2
Q1_WHT_DOMAIN             = le long de ne[0], par blocs contigus
Q1_WHT_BLOCK_SIZE         = 1024   [src/obrain-rotation.h:30 : #define OBRAIN_WHT_BLOCK 1024]
Q1_WHT_PADDING_RULE       = AUCUN padding ; GGML_ASSERT(ne00 % block == 0) le refuse
Q1_WHT_PERMUTATION        = aucune (papillon en place, ordre naturel)
Q1_WHT_SIGN_CONVENTION    = a+c / a-c (standard)
Q1_WHT_DTYPE_IN           = GGML_TYPE_F32
Q1_WHT_DTYPE_OUT          = GGML_TYPE_F32
Q1_WHT_ACCUMULATION_DTYPE = float (F32)
```

**Pourquoi 1024 et non la dimension entière** (`ggml.c:3006-3009`) : la WHT exige une puissance de
deux, et les largeurs du 27B hybride sont 5120 = 5×1024, 6144 = 6×1024, 17408 = 17×1024. Le
découpage en blocs est une **nécessité arithmétique**, pas un choix de performance — et c'est celui
de PrismML, dont le fichier déclare `block_size = 1024`.

`Q1_WHT_STRUCTURAL_INVARIANTS` (voir §25) découlent de ces valeurs, pas d'une supposition
d'orthonormalité générique : ici l'orthonormalité **est** établie par `1/sqrt(block)`.

---

## §5 — Côté d'application de la rotation

Source : `llama-graph.cpp:1487-1523`, commentaire de référence
`docs/TERNARISATION-2026-09-20.md`.

```
Q1_WHT_ACTIVATION_SIDE = RUNTIME, à l'activation, dans build_lora_mm
Q1_WHT_WEIGHT_SIDE     = OFFLINE, au repliement des poids (hors ligne, déjà dans le GGUF)
```

Le contrat est porté par le **TYPE** : un poids `TQ2_G128` est, par construction, replié hors ligne
par la même transformée. Un runtime qui ne connaît pas ce type refuse le fichier au lieu de le lire
de travers. Le runtime applique donc la transformée **à l'activation uniquement** — les deux
moitiés vont ensemble.

**Caching** : `obrain_rot` (map `src -> rotated`) garantit **une seule rotation par activation**.
Sans ce cache, le même tenseur était recalculé pour chaque consommateur — 3 024 nœuds WHT par passe
pour 108 activations distinctes, et 44 % du décodage. Le résultat est identique au bit.

**Conséquence pour le reconstructeur** : la référence Q1 doit quantifier l'activation **post-WHT**,
et non appliquer elle-même la transformée. La transformée est déjà faite par le graphe.

---

## §6 — Point de capture (audit source)

Source : `llama-graph.cpp:1480-1537`.

```
1509  f1a_capture_ctx * const f1a_cc = f1a_capture_get_ctx();
1510  ggml_tensor * const f1a_cur_raw = cur;          <- activation TELLE QU'ELLE ARRIVE (pre-WHT)
1511-1523  [bloc WHT]  cur = <post-WHT>               <- cur est REMPLACE
1526  // commentaire : "`cur` est ici EXACTEMENT l'activation consommee par le matmul
                       (brute ou repliee par WHT)"
1529  ggml_tensor * const f1a_cur_cap = ggml_dup(ctx0, cur);   <- DUP du POST-WHT
1534  f1a_capture_note(f1a_cc, w, f1a_cur_raw, cur, f1a_cur_cap, f1a_mm_raw, res);
```

```
WHT_INPUT_TENSOR                = f1a_cur_raw  (pre-WHT, tel qu'il arrive)
WHT_OUTPUT_TENSOR               = cur          (post-WHT)
NATIVE_MM_INPUT_TENSOR          = f1a_cur_cap  = ggml_dup(cur)   -> POST-WHT
CURRENT_CAPTURE_INPUT_TENSOR    = f1a_cur_cap
CURRENT_CAPTURE_IS_POST_WHT     = YES
```

```
Q1_CAPTURE_POINT = POST_WHT_TENSOR_ACTUALLY_CONSUMED_BY_NATIVE_MM
Q1_CAPTURE_POINT_IMPLEMENTATION_READY = YES
```

**Le bloc WHT s'exécute AVANT le bloc de capture.** Quand `ggml_dup` copie `cur`, `cur` vaut déjà
le tenseur post-WHT. Le payload `activation` du manifeste **est** l'activation post-WHT consommée
par le matmul natif.

Le manifeste conserve en outre `cur_raw_id` (pre-WHT), `cur_eff_id` (post-WHT) et
`rotation_applied = (cur_raw != cur_used)` — donc **les deux représentations sont traçables**, et
`rotation_applied=1` est une preuve par cible que la branche WHT a été prise.

`Q1_WHT_RUNTIME_PROOF = CAPTURE_MANIFEST_ROTATION_APPLIED_FLAG` (voir §28). Aucune instrumentation
nouvelle n'est requise : le champ existe déjà.

---

## §7 — Population WHT-éligible

Condition source (§3) : `w->type == GGML_TYPE_TQ2_G128`. La population Q0 a été construite sur
l'inventaire `TQ2_TENSOR_INVENTORY.csv`, dont chaque entrée est par construction un `TQ2_G128`.

```
Q1_WHT_ELIGIBLE_TENSOR_COUNT=168
Q1_WHT_ELIGIBLE_FFN_GATE=48
Q1_WHT_ELIGIBLE_FFN_UP=48
Q1_WHT_ELIGIBLE_ATTN_GATE=36
Q1_WHT_ELIGIBLE_ATTN_Q=12
Q1_WHT_ELIGIBLE_ATTN_K=12
Q1_WHT_ELIGIBLE_ATTN_OUTPUT=12
```

Inventaire déterministe : `Q1_WHT_ELIGIBLE_INVENTORY.csv`, gelé par SHA256 avant toute exécution.

`attn_k` est inclus : le §14 du brief Q0 avait déjà établi que sa sortie peut alimenter le KV cache
plutôt qu'un chemin direct vers les logits, ce qui ne l'exclut pas du test opérateur.

---

## §8 — Pipeline de référence

L'ordre est dérivé de la source, pas supposé :

```
Q1_REFERENCE_PIPELINE =
  1. lire le payload `activation` de la cible   = activation POST-WHT native
  2. quantifier q8_1 avec la sémantique native qualifiée par N4/N5 :
       d_fp32 = amax / 127.0f
       q      = (amax == 0) ? 0 : roundf(xi / d_fp32)     [roundf, PAS np.rint]
       stored_d = fp16(d_fp32)                             [PAS d inverse fp16]
  3. décoder les poids TQ2_G128 depuis le GGUF :
       tensor_start = align_up(metadata_end, gguf_alignment) + offset_relatif
       bloc de 34 octets : qs[32] ternaire + d:ggml_half ; 128 poids par bloc
       decodeur : 0 -> -1, 1 -> 0, 2 -> +1, 3 -> +1
  4. matmul natif : mm_base[r] = somme_blocs d16 * dq * somme dp4a(...)
  5. comparer à mm_capture = payload `base_output` de la MEME occurrence
```

**Aucune transformée WHT n'est appliquée par la référence.** Elle est déjà faite par le graphe, et
le payload la contient. Appliquer la WHT une seconde fois serait une erreur de représentation.

```
Q1_QTYPE43_DECODER_SOURCE_RECHECK = PASS   (vecdotq.cuh, table {0:-1, 1:0, 2:1, 3:1}, code 3 non produit)
NATIVE_Q8_INTERMEDIATE_COMPARISON = SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED
```

Cette dernière ligne est **inchangée** : les blocs q8_1 intermédiaires natifs n'ont pas été copiés
directement en Q0, et Q1 ne transforme pas ce fait en observation directe.

---

## §9 — Invariant de représentation

```
Q1_REPRESENTATION_ALIGNMENT_REQUIRED=YES

REFERENCE_REPRESENTATION == NATIVE_MM_REPRESENTATION == CAPTURE_REPRESENTATION
```

Aucune conversion « pratique » post hoc pour faire coïncider les résultats. L'alignement est déjà
garanti par construction : la référence consomme le payload post-WHT, qui EST la représentation
native consommée par le matmul.

---

## §10 — Sémantique OFF/ON

```
Q1_OFF = calcul POST-WHT  +  capture OFF     (OBRAIN_NO_ROTATION UNSET, OBRAIN_F1A_CAPTURE=0)
Q1_ON  = calcul POST-WHT  +  capture ON      (OBRAIN_NO_ROTATION UNSET, OBRAIN_F1A_CAPTURE=1)

Q1_OFF_WHT_ENABLED=YES
Q1_ON_WHT_ENABLED=YES
```

**Interdit** : `Q1_OFF = WHT désactivé`, `Q1_ON = WHT activé`. Cela mesurerait autre chose — ce
serait précisément la comparaison raw-vs-post-WHT que le §0 interdit.

La seule différence OFF/ON est **l'instrumentation de capture**.

```
Q1_OFF_CAPTURE_ACTIVITY_COUNT=0        (exigé)
```

**Ledger OFF :**
```
Q1_OFF_EVENT_LEDGER_REQUIRED=NO
```
Décision explicite, prise **avant** exécution. Motif : le contrôle OFF de Q1 vise uniquement
(i) l'inertie de la capture et (ii) la transparence bitwise. Le ledger sert la provenance
temporelle du run ON, seule population scientifique. Le protocole Q0 gelé avait déjà tranché ce
point pour son propre OFF ; Q1 hérite de la même décision pour la même raison, et la réénonce
explicitement plutôt que de la présumer.

---

## §11 — Transparence OFF/ON

Comparaison **bit à bit** des logits OFF et ON, les deux POST-WHT :

```
Q1_OFF_LOGITS_SIZE   == Q1_ON_LOGITS_SIZE
Q1_OFF_LOGITS_SHA256 == Q1_ON_LOGITS_SHA256
LOGIT_BITWISE_MISMATCH_COUNT = 0
```

Alors : `Q1_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES`

**Cela ne compare pas WHT vs non-WHT.** Les deux côtés ont la rotation activée.

**Interdit (§21)** : utiliser `Q0 logits` vs `Q1 logits` comme critère de PASS. Le Q0 est un
contexte descriptif, jamais un critère.

---

## §12 — Provenance temporelle

Q1 **réutilise le système runtime validé par Q0**. Aucun nouveau système de pairing.

Sur le run ON, exiger :

```
EVENT_LEDGER_IO_FAILURE_COUNT=0
EVENT_SEQUENCE_ID_DUPLICATE_COUNT=0
EVENT_SEQUENCE_GAP_COUNT=0
EVENT_SEQUENCE_MONOTONIC=YES
PREFILL_DECODE_STEP=-1
PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP=NO
SCIENTIFIC_DECODE_STEP_SET={0,1,2,3,4,5,6,7,8}
TOTAL_COMPUTE_SUBMIT_COUNT=10
PREFILL_COMPUTE_SUBMIT_COUNT=1
SCIENTIFIC_COMPUTE_SUBMIT_COUNT=9
COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=0
SYNC_COMPLETE_MISSING_EPOCH_COUNT=0
SYNC_COMPLETE_DUPLICATE_EPOCH_COUNT=0
SYNC_BEFORE_SUBMIT_COUNT=0
CAPTURE_EVENT_STEP_MISMATCH_COUNT=0
CAPTURE_EVENT_SUBMISSION_MISMATCH_COUNT=0
CAPTURE_EVENT_EPOCH_MISMATCH_COUNT=0
REFERENCE_SAMPLE_TEMPORAL_PAIRING=COMPLETE
LEDGER_MANIFEST_MISMATCH_COUNT=0
LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT=0
```

Rappel de sémantique gelée par Q0 : `is_scientific(step) := 0 <= step <= 8`. `-1` est un entier
valide **hors** population ; ne jamais utiliser « est un entier » comme discriminant.

---

## §13 — Contrôles de complétude et de contenu

```
MISSING_PAYLOAD_COUNT=0
SHA256_MISMATCH_COUNT=0
DIMENSION_MISMATCH_COUNT=0        (dimensions depuis l'inventaire factuel, PAS de whitelist)
NONFINITE_PAYLOAD_COUNT=0
ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=0
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=PASS
PAYLOAD_PROVENANCE_COMPLETE=YES
CAPTURE_CONTENT_VALIDITY=PASS
```

Le détecteur zero-input/nonzero-output est celui validé par le contrôle négatif de la clôture du
défaut 56. Il n'est pas requalifié.

---

## §14 — Sentinelles

Les 30 tenseurs de la sélection Q0 sont **tous** dans la population WHT-éligible (§7 : 168 = la
population Q0 entière). Aucun remplacement n'est nécessaire.

```
Q1_SENTINEL_TENSOR_COUNT=30
Q1_SENTINEL_SELECTION = <copie exacte de la sélection Q0>
Q1_SENTINEL_SELECTION_SHA256=448808715eea67d416be356d5f03e28b9675d15b05acb510e202a0f263697798

ffn_gate     8,20,32,44,55
ffn_up       8,20,32,44,55
attn_gate    8,20,32,44,54
attn_q       11,23,35,47,55
attn_k       11,23,35,47,55
attn_output  11,23,35,47,55
```

**Règle de remplacement** (à n'appliquer que si une sentinelle s'avérait non éligible, ce qui **n'est
pas le cas** — elle est enregistrée pour supprimer tout degré de liberté) :

```
same role
  -> ordre croissant de layer_index
  -> premier non déjà sélectionné
  -> jamais après observation numérique
```

---

## §15 — Budget numérique

```
Q1_RELATIVE_L2_THRESHOLD      = 1e-3
Q1_COSINE_THRESHOLD           = 0.9999
Q1_MAX_ABS_THRESHOLD_FORMULA  = 1e-2 * max_abs(native_mm_base)
Q1_MAX_ABS_REFERENCE_DENOMINATOR = max_abs(mm_capture de la MEME occurrence)
```

**Résolution de l'ambiguïté `native_mm_base` avant toute mesure.** Le dénominateur est
`max_abs(mm_capture)` — le payload `base_output` de la même occurrence temporelle, c'est-à-dire la
sortie native **dans le même espace de représentation que la référence** (POST-WHT). Il n'est PAS
tiré d'un tenseur pré-WHT et il n'est PAS agrégé entre occurrences.

Budget **identique à Q0**. Aucune adaptation après observation.

`COSINE` brut conservé tel quel dans les CSV, y compris une excursion légère au-dessus de 1 par
arrondi FP ; le critère s'applique à l'interprétée `clamp(raw, -1, 1)`. Le fichier n'est jamais
réécrit pour clamper.

---

## §16 — Invariants structurels WHT

Définis **uniquement** parce que la source établit `norm = 1/sqrt(block)` (orthonormale) :

```
Q1_WHT_STRUCTURAL_INVARIANTS:
  I1  determinisme       : deux exécutions de ggml_wht sur la meme entree donnent le meme bit
  I2  roundtrip          : WHT(WHT(x)) == x a 1e-6 pres (auto-inverse, orthonormale)
  I3  norme              : ||WHT(x)||_2 == ||x||_2 a 1e-5 pres (orthonormale)
  I4  bloc               : ne[0] % 1024 == 0 sur toute activation rotative (deja un GGML_ASSERT)
```

Ces invariants ne sont **pas** des substituts du budget matmul : ils sont des contrôles
indépendants. Ils sont facultatifs au sens où leur échec n'entraîne pas `Q1_FORWARD_STATUS=FAIL`
automatique — il entraîne un `BLOCKED` sur `Q1_WHT_SEMANTICS`, car il signalerait que la
compréhension du chemin est incomplète.

---

## §17 — Population exhaustive

```
Q1_POPULATION_FORMULA =
  ALL Q1_WHT_ELIGIBLE TARGETS
    x
  ALL REQUIRED SCIENTIFIC CAPTURE OCCURRENCES

Q1_REQUIRED_SAMPLE_COUNT =
  (# identites de cible eligibles effectivement presentes dans le manifeste ON)
    x
  (# occurrences scientifiques requises lues dans le ledger ON, selon la regle gelée §12)
```

**Aucun nombre n'est gelé ici.** Le nombre est **dérivé après lecture du ledger**, par cette
formule, et non pré-déclaré. Le protocole gèle la **règle**, pas la cardinalité.

Une identité de cible × une occurrence scientifique = un échantillon distinct.

---

## §18 — Ordre des gates (gelé)

```
A. authority hashes            (5 rechecks + inventaire + sentinelles + source provenance)
B. WHT runtime activation proven
C. temporal provenance
D. capture completeness / content validity
E. OFF/ON transparency (les DEUX POST-WHT)
F. sentinel POST-WHT native reference
G. exhaustive POST-WHT native reference
H. Q1_FORWARD_STATUS
```

**Aucun numérique (F, G) tant que A–E n'ont pas tous passé.**

---

## §19 — Preuve d'activation WHT au runtime

Le simple fait que `OBRAIN_NO_ROTATION` soit absent ne suffit pas.

```
Q1_WHT_RUNTIME_PROOF = CAPTURE_MANIFEST_ROTATION_APPLIED_FLAG
Q1_WHT_REQUIRED_EXECUTION_COUNT > 0
Q1_WHT_REQUIRED_EXECUTION_COUNT = nombre de lignes du manifeste ON avec rotation_applied=1
```

Le champ `rotation_applied` existe déjà dans `GRAPH_CAPTURE_MANIFEST.csv` (source :
`f1a-capture.cpp`, `r.rotation_applied = (cur_raw != cur_used)`), et vaut `1` exactement quand la
branche WHT a remplacé l'activation. **Aucune instrumentation nouvelle n'est requise**, donc
`PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0` est préservable.

Gate : au moins une cible du run ON porte `rotation_applied=1`. En pratique toutes doivent le
porter, puisque toute cible éligible est `TQ2_G128` et que la variable est absente — un
`rotation_applied=0` sur une cible éligible serait une anomalie et doit être signalée.

---

## §20 — Provenance source et build

Q1 gèle la **règle** de provenance, pas un hash binaire futur :

```
Q1_SOURCE_PROVENANCE.txt : pour chaque fichier déterminant
  path, sha256, symboles pertinents
  - llama-graph.cpp   build_lora_mm, bloc WHT, bloc capture
  - obrain-rotation.h OBRAIN_WHT_BLOCK
  - ggml.c            ggml_wht
  - ggml-cpu/ops.cpp  ggml_compute_forward_wht_f32   (semantique d'autorite)
  - ggml-cuda/wht.cu  noyau CUDA (copie declaree de la semantique CPU)
  - vecdotq.cuh       decodeur qtype43
  - f1a-capture.cpp   capture, manifeste, provenance, ledger
  - f1a-eval.cpp      harnais, boucle scientifique
  - llama-context.cpp hook de soumission, fin d'epoque

Avant execution, exiger :
  F1A_EVAL_SHA256, LIBLLAMA_SHA256, config de build CUDA, definitions de compilation
  pertinentes, chemin de la lib linkee au runtime, SHA256 de cette lib.
```

---

## §21 — Taxonomie PASS / FAIL / BLOCKED

**PASS** seulement si **tous** :

```
5 authority hashes PASS
Q1_WHT_RUNTIME_ACTIVATION_PROVEN=YES
REFERENCE_SAMPLE_TEMPORAL_PAIRING=COMPLETE
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=PASS
CAPTURE_CONTENT_VALIDITY=PASS
Q1_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
LOGIT_BITWISE_MISMATCH_COUNT=0
Q1_SENTINEL_SUBSET_VALIDATION=PASS      et SENTINEL_FAILED_SAMPLE_COUNT=0
Q1_FULL_NATIVE_REFERENCE_VALIDATION=PASS et FULL_FAILED_SAMPLE_COUNT=0
```

**BLOCKED** — la mesure valide ne peut pas être établie :
autorité manquante · provenance manquante · pairing ambigu · instrumentation runtime incomplète ·
capture incomplète · job interrompu · référence non évaluable · sémantique WHT incomprise.

**FAIL** — mesure complète et valide qui viole un budget pré-déclaré ou une exigence de
transparence.

---

## §22 — Interdictions post-hoc

```
NO_THRESHOLD_CHANGES_AFTER_FIRST_Q1_MEASUREMENT
NO_SENTINEL_CHANGES_AFTER_FIRST_Q1_MEASUREMENT
NO_POPULATION_REDEFINITION_AFTER_FIRST_Q1_MEASUREMENT
NO_RAW_VS_POST_WHT_PASS_CRITERION
NO_EXCLUSION_OF_FAILED_SAMPLES_POST_HOC
NO_RETOKENIZATION
NO_RESULT-DRIVEN_CAPTURE_POINT_CHANGE
```

Toute modification nécessaire impose une **version v2** avec un nouveau gel explicite. Aucune
correction silencieuse de v1.

---

## §23 — Historique immuable

```
Q0_FORWARD_STATUS=PASS                    (prospectif)
Q0_PREDECLARED_PROTOCOL_FOUND=NO
Q0_PREDECLARED_STATUS=NOT_EVALUABLE_NO_PREDECLARED_PROTOCOL
Q0_HISTORICAL_STATUS=NOT_PASSED
Q1_HISTORICAL_ALLOWED=NO
NATIVE_Q8_INTERMEDIATE_COMPARISON=SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED
N3NONIES_VS_N4_OLD_METRIC_DIFFERENCE=NOT_FULLY_RECONSTRUCTED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
```

Rien de tout cela n'est réécrit, quel que soit le résultat de Q1.
