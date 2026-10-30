# ENGINE-007B-F1A-Q0 — SMOKE CAPTURE-OFF ET CAPTURE NO-ROTATION : ÉTAT

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0/`

---

## 0 — Résultat

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
```

**Le smoke capture-off passe.** **Q0 capture-on échoue**, à un endroit précis et instructif.

| étape | état |
|---|---|
| **Q0-S0** smoke capture désactivée | **PASS** |
| Q0-S0 compteurs d'inertie runtime | **NON MESURÉS** — non implémentés |
| Q0-S1 trajectoire figée | **FAIT** — 8 tokens |
| **Q0-S2** Q0 OFF | **PASS** — logits réels produits |
| **Q0-S3** Q0 ON | **ÉCHEC** — SIGABRT à l'extraction |
| Q0-S4 à S6 | non atteints |

---

## 1 — D'abord un défaut de MON harnais, qui a produit deux fois 5 Go

Je dois commencer par ça, parce que c'est le premier résultat de cette passe et qu'il est mauvais.

J'ai lancé le CLI `llama-obrain` en redirigeant sa sortie vers un fichier, depuis un heredoc SSH.
**Le CLI est resté en mode INTERACTIF et a lu le heredoc** : mes propres commandes shell sont
devenues ses entrées, et il a généré du texte dessus.

```
tentative 1 : SMOKE_CAPTURE_OFF.txt = 5 524 130 360 octets
tentative 2 : (apres un correctif qui ne corrigeait rien) 4 295 057 795 octets
   -> disque monte a 92 pourcent ; les deux fichiers ont ete supprimes
   -> AUCUN resultat exploitable : ces executions ne mesuraient rien
```

Ce que la sortie montrait, une fois comprise : `[ Prompt: 120.5 t/s | Generation: 18.3 t/s ]`
(le modèle générait), l'erreur `does not match the expected peg-native format` (le parseur de chat
rejetait), et `> echo "RC=$?"` (mes commandes étaient l'entrée).

```
CAUSE=le CLI applique un template de chat 'peg-native' et boucle, meme avec
      --no-conversation, -no-cnv ET stdin ferme
```

**Leçon** : j'ai d'abord interprété la sortie vide comme un problème du modèle ou du build
diagnostique. C'était mon harnais. Le fichier de 5 Go était le symptôme que j'aurais dû voir
immédiatement.

## 2 — Correctif : un harnais minimal qui n'utilise pas le CLI

`tools/f1a-eval.cpp` — utilise **libllama directement** :

```
une SEQUENCE DE TOKENS FIGEE (teacher forcing : on force la suite, on ne l echantillonne pas)
un seul token par appel llama_decode -- le regime de decodage que 007A mesure
logits ecrits en binaire ; aucune machinerie de chat, aucun sampling, aucune generation
SHA256 binaire = 1c340a9546c3df584b80ba5123b5491d907d0051639f3404599c9e8c11298384
```

Erreur de compilation rencontrée : `llama_context_params` n'a pas de champ `logits_all` dans ce
fork. Champ retiré, une ligne.

## 3 — Q0-S0 : smoke capture désactivée — **PASS**

```
OBRAIN_NO_ROTATION=1, OBRAIN_F1A_CAPTURE non pose
trajectoire figee : 8 tokens = 1 prefill + 7 pas de decodage, un token par appel
RC=0
OFF_logits.bin = 6 952 976 octets  (16 octets d entete + 7 pas x 248320 x 4)
repertoire de capture : INEXISTANT -- aucun payload, aucun manifeste

CAPTURE_DISABLED_EXECUTION=PASS
NO_CAPTURE_PAYLOADS=YES        NO_CAPTURE_MANIFEST_ENTRIES=YES
```

**Réserve que je dois signaler** : votre §4 exige des compteurs runtime prouvant l'inertie du
chemin, et interdit explicitement de l'inférer de l'absence de fichier. Ces compteurs ne sont pas
implémentés.

```
CAPTURE_DISABLED_RUNTIME_PATH_INERT=NON_MESURE
```

## 4 — Q0-S3 : la capture s'exécute, puis s'arrête sur un point réel

**Ce qui fonctionne, prouvé par l'exécution :**

```
[f1a] inventaire charge : 168 noms eligibles, 0 doublons
   INVENTORY_FILE_SHA256=a8f21cc88cdf56165b127d41fe0940b5740e5e894688ef74d10902392b4eae9e
   INVENTORY_LOADED_NAME_COUNT=168     INVENTORY_DUPLICATE_NAME_COUNT=0
un payload a ete ecrit : capout-on/ep0_t1_activation.bin
```

Donc `f1a_eligible()` reconnaît bien les poids TQ2_G128 **par leur nom dans l'inventaire figé**, et
`f1a_capture_note()` enregistre bien une invocation cible. La chaîne identification → registre →
extraction est vivante.

**L'échec, avec sa pile exacte :**

```
RC=134 (SIGABRT), apres la premiere epoque de capture
   ggml/src/ggml-backend.cpp:342
   GGML_ASSERT(buf != NULL && "tensor buffer not set") failed
   pile : f1a_capture_end_epoch -> ggml_backend_tensor_get -> ggml_abort
```

## 5 — La cause, et ce qu'elle m'apprend sur ma propre méthode

`ggml_backend_tensor_get()` est appelé sur un tenseur **dont le buffer n'est pas alloué**. Mes
tenseurs de capture sont des nœuds de graphe dont les valeurs sont produites pendant l'exécution,
mais le planificateur ne leur garantit pas un buffer **lisible côté hôte** au moment où j'extrais.

**C'est exactement ce que la preuve statique de I2-b ne pouvait pas couvrir.**

Ma preuve de I2-b établissait l'**ordre** :

```
COMPUTE < SYNCHRONIZE < EXTRACTION < INVALIDATION
```

Elle ne disait rien de la **résidence** du buffer. Ce sont deux questions distinctes, et je les
avais traitées comme une seule — j'ai présenté `CAPTURE_TENSOR_LIFETIME_STATIC_PROOF=PASS` d'une
manière qui laissait croire que la durée de vie était réglée. Elle ne l'était qu'à moitié : l'ordre
était prouvé, la résidence ne l'était pas.

```
CAPTURE_TENSOR_LIFETIME_PROVEN=NO
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
```

## 6 — Ce qu'il faut pour débloquer

```
1. verrouiller les tenseurs de capture sur un backend lisible par l hote
   (ggml_backend_sched_set_tensor_backend vers le backend CPU, ou un buffer type hote)
2. ou extraire depuis le backend REEL du tenseur, pas depuis un backend suppose :
   ggml_backend_sched_get_tensor_backend(sched, t), puis synchroniser CE backend
   -- mon code fait ggml_backend_sched_get_backend(sched, 0), ce qui suppose que l indice 0
      est le bon. C est une supposition que je n ai pas verifiee.
3. confirmer ensuite que la taille obtenue correspond a ggml_nbytes(t)
4. implementer les compteurs d inertie du paragraphe 4
```

Le point 2 est probablement le correctif : j'ai supposé un backend au lieu de le **demander au
tenseur**. C'est la même erreur de méthode que celle qui a produit le fichier de 5 Go — supposer au
lieu de vérifier.

## 7 — Ce que Q0 n'a pas produit

Aucun des gates de Q0 n'est franchi au-delà du smoke : pas de manifeste exploitable, pas de
comparaison OFF/ON, pas de gate de forme, pas de contrôle de finitude, pas de validation de la
référence contre la sortie native. `Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR` n'est pas
mesuré.

```
Q0_CAPTURE_COMPLETENESS=NOT_MEASURED
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NOT_MEASURED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
```

## 8 — Rapport final (§42)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_I2_STATUS=BUILD_QUALIFIED_Q0_BLOCKED_ON_BUFFER_RESIDENCE
DIAGNOSTIC_BINARY_SHA256=b5144edee5cc88135c2d8e3baa064e2f243c7d141db3e6d75aadd7c0b11cb5b5
EVAL_HARNESS_SHA256=1c340a9546c3df584b80ba5123b5491d907d0051639f3404599c9e8c11298384
CAPTURE_PATCH_SHA256=1f490a92ff1d1767fedd64094962c6714d00aae1bcceed31a385a54467808eae
CAPTURE_DISABLED_EXECUTION=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=NON_MESURE
Q0_SEQUENCE_COUNT=1
Q0_DECODE_TOKEN_COUNT=7
Q0_PREFILL_TOKEN_COUNT=1
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
CAPTURE_GRAPH_DEPENDENCY_PROVEN=NO
TARGET_MATMUL_USES_CAPTURE_TENSOR_AS_SRC1=YES_PAR_CONSTRUCTION_NON_CONFIRME_A_L_EXECUTION
DOWNSTREAM_USES_CAPTURED_BASE_OUTPUT=YES_PAR_CONSTRUCTION_NON_CONFIRME_A_L_EXECUTION
CAPTURE_TENSOR_ALIASES_SOURCE_STORAGE=NO_PAR_CONSTRUCTION
CAPTURE_TENSOR_MATERIALIZED_BY_GRAPH=YES
CAPTURE_TENSOR_LIFETIME_PROVEN=NO
CAPTURE_TENSOR_BUFFER_RESIDENCE_PROVEN=NO
EXPECTED_GRAPH_TARGETS=NOT_MEASURED
OBSERVED_EXTRACTED_TARGETS=1_PARTIEL_PUIS_ABANDON
MISSING_CAPTURE_COUNT=NOT_MEASURED
UNEXPECTED_CAPTURE_COUNT=NOT_MEASURED
DUPLICATE_TARGET_CAPTURE_COUNT=NOT_MEASURED
TARGET_INVOCATION_COUNT=NOT_MEASURED
UNIQUE_ACTIVATION_CAPTURE_COUNT=NOT_MEASURED
ROTATION_APPLIED_TARGET_COUNT=NOT_MEASURED
ROTATION_NOT_APPLIED_TARGET_COUNT=NOT_MEASURED
CAPTURED_E5_ACTIVATION_K_MATCH=NOT_MEASURED
CAPTURED_E6_ACTIVATION_K_MATCH=NOT_MEASURED
ACTIVATION_NAN_COUNT=NOT_MEASURED
ACTIVATION_INF_COUNT=NOT_MEASURED
BASE_OUTPUT_NAN_COUNT=NOT_MEASURED
BASE_OUTPUT_INF_COUNT=NOT_MEASURED
LOGICAL_SERIALIZATION_VALIDATED=NO
Q0_CAPTURE_ON_OFF_LOGITS_BITWISE_MATCH=NOT_MEASURED
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NOT_MEASURED
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
REFERENCE_ERROR_BUDGET=REL_L2_LE_1e-3_AND_COSINE_GE_0.9999_AND_MAX_ABS_LE_1e-2_TIMES_NATIVE_MAXABS
REFERENCE_ERROR_BUDGET_NOT_JUSTIFIED_BY_KERNEL_WALL=YES
REFERENCE_VALIDATION_MAX_ABS=NOT_MEASURED
REFERENCE_VALIDATION_MEAN_ABS=NOT_MEASURED
REFERENCE_VALIDATION_RELATIVE_L2=NOT_MEASURED
REFERENCE_VALIDATION_COSINE=NOT_MEASURED
Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED
REFERENCE_ARITHMETIC_MODEL=SOURCE_CONFIRMED_NOT_NATIVE_VALIDATED
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
ACTIVATION_CAPTURE_RUN_PERFORMANCE_DATA=INVALID_NOT_MEASURED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_Q0_BIS_FIX_BUFFER_RESIDENCE_THEN_RERUN
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 9 — Défauts de harnais de cette passe

Je n'assigne pas de numéro sans avoir vérifié le registre complet, comme le §37 l'exige. Les
défauts constatés, à numéroter à la prochaine passe :

```
DEFAUT A  CLI llama-obrain utilise pour une qualification deterministe : il boucle en mode
          interactif et consomme stdin, produisant 5,5 Go puis 4,3 Go de sortie parasite.
          RESOLU : harnais minimal libllama ecrit pour Q0.
DEFAUT B  llama_context_params::logits_all inexistant dans ce fork.
          RESOLU : champ retire.
DEFAT C   backend suppose au lieu d etre demande au tenseur dans f1a_capture_end_epoch
          (ggml_backend_sched_get_backend(sched, 0)). CAUSE du SIGABRT.
          NON RESOLU -- c est la prochaine action.
```

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_I2_STATUS=BUILD_QUALIFIED_Q0_BLOCKED_ON_BUFFER_RESIDENCE
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_EXECUTION=PASS
CAPTURE_TENSOR_LIFETIME_STATIC_PROOF=PASS
CAPTURE_TENSOR_LIFETIME_PROVEN=NO
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
TQ2_G128_NATIVE_ACTIVATION_DOMAIN=POST_WHT
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
HARNESS_DEFECT_46=RESOLVED
REFERENCE_ERROR_BUDGET_NOT_JUSTIFIED_BY_KERNEL_WALL=YES
SALIENCY_MEASURED=NO
EPISODE_ALIGNED_STRUCTURED_ELIMINATION=NOT_DETERMINED
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
