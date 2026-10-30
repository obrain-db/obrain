# ENGINE-007B-F1A-Q0-N1 — QUALIFICATION NUMÉRIQUE SANS WHT

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n1/`

---

## 0 — Résultat

**Le gate de transparence numérique passe, brillamment. Deux gates restent ouverts, à cause d'un
défaut de mon instrumentation.**

```
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
BLOCAGE=BLOCKED_ACTIVATION_CAPTURE_GATE
```

| gate | état |
|---|---|
| **N1-A** capture OFF | **PASS** — cinq zéros vérifiés |
| **N1-C** transparence numérique OFF/ON | **PASS — BITWISE** |
| **N1-B** complétude de capture | **NON QUALIFIÉE** — mon manifeste s'écrase |
| **N1-E** population de validation | figée mais **non résolue** |
| **N1-F** reconstruction de l'opérateur | **NON MESURÉE** |

## 1 — Le résultat le plus fort : transparence bit à bit

```
LOGIT_BITWISE_MATCH = YES
2 234 880 valeurs identiques BIT A BIT   (9 pas x 248 320)
NaN = 0    Inf = 0
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT = YES
```

La greffe diagnostique — deux `ggml_dup` par projection, 168 cibles par pas — **ne change pas un
seul bit** de la sortie du modèle sur 9 pas de décodage. C'est le résultat le plus favorable
possible : le §5 exigeait la correspondance bitwise comme critère primaire, et elle est obtenue.

Je n'ai donc **pas** eu à recourir à un critère de tolérance, ni à accepter une différence
« parce qu'elle semble petite ».

## 2 — Entrées figées avant mesure (§1, §2)

```
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
MODEL_BYTES=12844231424
DIAGNOSTIC_BINARY_SHA256=e4b894287e1cc90cef1097253e9fb3115043bc1eeb965b8890c1c611e778cd7c
LIBLLAMA_SHA256=65fd4b6e373119d8d49570991035fca014dcd990af25a8c3b2a5717520998042

Q0_TOKEN_TRAJECTORY sha256=dc8d54a929cfeb42646b099cc3e0af61b8d8a13e5234d748259e225c9d86f44b
tokens = 9906 279 6511 314 9338 369 220 17 1879 323   (1 prefill + 9 pas de decodage)
SEQUENCE_COUNT=1   DOMAIN=DECODE_ONLY   n_tokens_par_decode=1
OFF : OBRAIN_NO_ROTATION=1, OBRAIN_F1A_CAPTURE unset
ON  : OBRAIN_NO_ROTATION=1, OBRAIN_F1A_CAPTURE=1
```

`OFF_ON_TOKEN_TRAJECTORY_MATCH=YES` — les deux exécutions consomment la **même** trajectoire figée.
Les tokens ont été figés **avant** toute mesure, donc aucun n'a été choisi en fonction des logits.

## 3 — Capture OFF (N1-A)

```
[f1a-ctr_standalone] ctx_active=0 probe=4493 eligible_enter=0 note=0
                     extract_call=0 payload_write=0 dup_added=0

CAPTURE_DISABLED_EXECUTION=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
```

Cinq zéros, plus `dup_added=0`. Le fichier de logits est produit par le **même mécanisme
d'observation** dans les deux modes : `f1a-eval` écrit les logits depuis `llama_get_logits_ith`,
indépendamment des `DUP` diagnostiques.

## 4 — Capture ON (N1-B) : le défaut

```
[f1a-sub] submission=1..4  gf=0xfc7017517030  nodes=3991  rc=0  fp=9330c682f394d3bb
[f1a-eff] OUTPUT_DEPENDENT=156  KV_SIDE_EFFECT=12  UNEXPLAINED=0
[f1a-ctr_standalone] ctx_active=4495 note=1512 extract_call=672 payload_write=672
```

672 payloads écrits, classification 156/12/0 reproduite, les quatre soumissions acceptées avec la
même empreinte topologique.

**Mais :**

```
GRAPH_CAPTURE_MANIFEST.csv : 337 lignes
672 payloads sur disque, 2 epoques
```

**Chaque époque rouvre le manifeste en `"w"` et écrase la précédente.** Le manifeste ne décrit donc
que la dernière époque, et je ne peux pas apparier les payloads à leurs cibles — ni vérifier
`CAPTURED_E5/E6_ACTIVATION_K_MATCH`, ni la finitude par cible, ni la sérialisation logique.

```
HARNESS_DEFECT_52=GRAPH_CAPTURE_MANIFEST_OVERWRITTEN_EACH_EPOCH
```

À noter : les manifestes de **résidence** et de **provenance** ne sont pas touchés — le premier est
rouvert en `"w"` mais **à l'intérieur** de chaque époque (donc par époque, pas écrasé entre elles
dans la mesure où je les lis séparément), le second est ouvert en `"a"`. C'est bien
`GRAPH_CAPTURE_MANIFEST.csv` qui perd une époque.

## 5 — N1-E / N1-F : non résolus

La sélection a été figée **avant** tout calcul d'erreur, conformément au §7 :

```
blk.20.ffn_up.weight       FFN_E5         tid=44
blk.16.attn_gate.weight    ATTENTION_E5   tid=29
blk.16.attn_output.weight  ATTN_OUTPUT_E6 tid=NON TROUVE
Q0_REFERENCE_SELECTION.sha256=32206039d290d4a72afb9e274f3e2652bf9afa809c9f4c14ef5a3e55cdf43adc
```

**`blk.16.attn_output.weight` a 0 occurrence** dans le manifeste exploitable. La couverture E6 de
cette exécution **n'est pas établie**, et je ne peux pas honnêtement sélectionner un tenseur E6 que
je ne retrouve pas.

```
Q0_CAPTURE_COMPLETENESS=NON_QUALIFIEE
Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NON MESUREE
```

La reconstruction `q8_1 × TQ2_G128` contre `mm_base` n'a **pas** été calculée. Le script est écrit
et la chaîne de mesure est prête (lecture des poids TQ2_G128 depuis le GGUF par offset, quantifie
l'activation capturée, décode les codes ternaires, ordre `dp4a` → double échelle fp16), mais sans
appariement payload ↔ cible il n'a rien à comparer.

## 6 — Deux erreurs de méthode que je dois déclarer

**Première.** J'ai écrit un harnais d'analyse qui a produit des résultats absurdes
(`awk` sur le mauvais index de colonne, `target_id` extrait du mauvais fichier). J'ai d'abord lu ces
résultats comme un problème de capture. C'était **mon analyse** qui était fausse — la capture
fonctionnait.

**Seconde.** En R6 j'avais déjà interprété une forme inattendue comme un défaut (le « décalage
d'une soumission »), avant de la retirer après mesure. Ici, même schéma : j'ai conclu depuis un
résultat d'analyse non vérifié.

Le point commun : **je conclus trop vite depuis une apparence.** La discipline qui a fonctionné à
chaque fois dans cette série — mesurer avant de conclure — je ne l'ai pas appliquée à mes propres
outils d'analyse.

## 7 — Rapport final (§14)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
DIAGNOSTIC_BINARY_SHA256=e4b894287e1cc90cef1097253e9fb3115043bc1eeb965b8890c1c611e778cd7c
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
Q0_SEQUENCE_COUNT=1
Q0_DECODE_TOKEN_COUNT=9
OFF_ON_TOKEN_TRAJECTORY_MATCH=YES
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
Q0_CAPTURE_COMPLETENESS=NOT_QUALIFIED
Q0_PAYLOAD_SHA256_VERIFIED=YES_POUR_L_EXECUTION_R6
ROTATION_APPLIED_TARGET_COUNT=NON MESURE_DANS_CETTE_EXECUTION
CAPTURED_E5_ACTIVATION_K_MATCH=NON MESURE
CAPTURED_E6_ACTIVATION_K_MATCH=NON MESURE
ACTIVATION_NAN_COUNT=NON MESURE
ACTIVATION_INF_COUNT=NON MESURE
BASE_OUTPUT_NAN_COUNT=NON MESURE
BASE_OUTPUT_INF_COUNT=NON MESURE
Q0_CAPTURE_ON_OFF_LOGITS_BITWISE_MATCH=YES
LOGIT_MAX_ABS_DIFF=0
LOGIT_MEAN_ABS_DIFF=0
LOGIT_RELATIVE_L2=0
LOGIT_COSINE=1
ARGMAX_AGREEMENT=9/9
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
Q8_1_REFERENCE_UNIT_TESTS=PASS_ACQUIS_NON_REJoue
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
REFERENCE_ERROR_BUDGET=REL_L2_LE_1e-3_AND_COSINE_GE_0.9999_AND_MAX_ABS_LE_1e-2_TIMES_NATIVE_MAXABS
REFERENCE_VALIDATION_MAX_ABS=NON MESURE
REFERENCE_VALIDATION_MEAN_ABS=NON MESURE
REFERENCE_VALIDATION_RELATIVE_L2=NON MESURE
REFERENCE_VALIDATION_COSINE=NON MESURE
Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NON MESURE
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
HARNESS_DEFECT_52=GRAPH_CAPTURE_MANIFEST_OVERWRITTEN_EACH_EPOCH
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=N1_BIS_FIX_MANIFEST_APPEND_THEN_REFERENCE_VALIDATION
```

## 8 — Livrables réellement produits

```
Q0_TOKEN_TRAJECTORY.txt / .sha256         trajectoire figee avant mesure
Q0_OFF_LOGITS.bin                         8 939 536 octets (16 + 9 x 248 320 x 4)
Q0_ON_LOGITS.bin                          meme taille
Q0_OFF_ON_NUMERIC_COMPARISON.csv          bitwise_match=YES, 0 NaN, 0 Inf
Q0_OFF_ON_TOKEN_EQUIVALENCE.txt           trajectoire identique + invariant de selection
Q0_OFF_RUNTIME_COUNTERS.txt               les cinq zeros
Q0_N1_STATUS.txt                          gates franchis et non franchis
N1_BUILD_AND_BINARY_PROVENANCE.txt        modele, binaire, bibliotheque, harnais
N1_CONFIG_PROVENANCE.txt                  environnements OFF/ON
Q0_REFERENCE_SELECTION.txt / .sha256      selection figee avant calcul
Q0_NATIVE_OPERATOR_REFERENCE_VALIDATION.csv   en-tete + statut PAYLOAD_MISSING
capoff/ capon/                            672 payloads binaires
off.txt on.txt                            journaux bruts
```

**Non produits** : `Q0_CAPTURE_MANIFEST.csv` (appariement impossible),
`Q0_CAPTURE_COMPLETENESS.txt`, `Q0_CAPTURE_SANITY.txt`, `Q8_1_REFERENCE_TESTS.txt`,
`TQ2_G128_REFERENCE_TESTS.txt`, `Q0_REFERENCE_AGGREGATES.txt`.

## 9 — Ce qui reste, précisément

```
N1-bis-1  corriger GRAPH_CAPTURE_MANIFEST.csv : l ouvrir en "a" (ou par epoque dans un
          fichier distinct). Aucune autre modification d instrumentation.
N1-bis-2  re-executer ON sur la MEME trajectoire figee, puis apparier payloads et cibles.
N1-bis-3  verifier K, episode_count, finitude, contiguite par cible.
N1-bis-4  resoudre attn_output E6 (ou documenter pourquoi il n est pas capture).
N1-bis-5  calculer la reconstruction reference contre mm_base, budget preenregistre applique
          PAR TENeur et non sur un agregat.
```

Le gate le plus difficile — la transparence bit à bit — **est franchi**. Ce qui reste est de la
plomberie de manifeste et l'exécution du calcul de référence.

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
Q0_CAPTURE_ON_OFF_LOGITS_BITWISE_MATCH=YES
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
OFF_ON_TOKEN_TRAJECTORY_MATCH=YES
Q0_CAPTURE_COMPLETENESS=NOT_QUALIFIED
Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NON MESURE
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
HARNESS_DEFECT_50=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
HARNESS_DEFECT_51=RETRACTED_AFTER_MEASUREMENT
HARNESS_DEFECT_52=OPEN_GRAPH_CAPTURE_MANIFEST_OVERWRITTEN_EACH_EPOCH
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
