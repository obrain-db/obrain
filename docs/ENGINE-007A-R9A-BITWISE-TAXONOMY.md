# ENGINE-007A-R9A — TAXONOMIE DES MISMATCHES BIT À BIT : LE MISMATCH N'EXISTAIT PAS

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r9a/`

---

## 0 — Résultat

Le brief demandait de résoudre une incohérence : `1875` mismatches bit à bit avec
`MAX_ABS_ERROR == 0`. **La réponse est que les 1875 « mismatches » étaient des `NaN` des deux côtés,
et que ces `NaN` venaient de mesh données de test, pas des noyaux.**

```
MISMATCH_TOTAL=0
NATIVE_NAN_COUNT=0        R8_NAN_COUNT=0
BITWISE_KERNEL_OUTPUT_MATCH=YES
```

Avec des entrées saines, **`K_FULL` natif et le noyau R8 produisent des sorties bit à bit
identiques sur les 5120 lignes.**

Et le noyau R6 **avec** l'artefact, sur les mêmes données saines :

```
R6_PROCESS_ID=99  (e7a-r6-clean, generateur corrige)
P12_VS_K_FULL_OUTPUT_MISMATCH=0
P12_KBX_SEQUENCE_MATCH_NATIVE=YES
```

```
ENGINE007A_R9A_STATUS=CLOSED_MISMATCH_WAS_INSTRUMENTATION_DATA_DEFECT
HARNESS_DEFECT_40=FAULTY_WEIGHT_DATA_GENERATOR_PRODUCED_NONFINITE_SCALE_VALUES
```

---

## 1 — La cause : mon générateur de données écrivait des `NaN` dans les échelles

Tous mes microbenchs depuis R2 remplissaient les poids octet par octet :

```cpp
for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);
```

Or `block_tq2_g128` se termine par un champ d'échelle **`uint16_t d`** interprété comme un
**demi-flottant**. Les 2 derniers octets de chaque bloc de 34 recevaient donc des bits arbitraires,
qui tombent régulièrement dans les plages `NaN`/`Inf` du format half. Le noyau fait
`acc += __half2float(bq->d) * sumi` : une échelle `NaN` **contamine toute la ligne**.

Preuve du mécanisme, obtenue en comparant les deux régimes sur le même binaire :

| | entrées d'origine | entrées corrigées |
|---|---|---|
| `MISMATCH_TOTAL` | 1875 | **0** |
| `NATIVE_NAN_COUNT` | **1875** | **0** |
| `R8_NAN_COUNT` | **1875** | **0** |

**Les deux noyaux ne divergeaient pas : ils produisaient tous les deux `NaN` sur les mêmes 1875
lignes.** Ma comparaison `if (a[i] != b[i])` comptait alors chaque ligne `NaN` comme un mismatch.
C'est le cas D du §12 du brief — `OUTPUT_COMPARISON_HARNESS_DEFECT` — sauf que le défaut n'était pas
dans la comparaison mais en amont, dans la génération des entrées.

## 2 — Portée : toutes les passes de R2 à R8

```
e7a-scaling.cu   (R2)   FAUTIF
e7a-r2.cu        (R2)   FAUTIF
e7a-r3.cu        (R3)   FAUTIF
e7a-r4.cu        (R4)   FAUTIF
e7a-r5.cu        (R5)   FAUTIF
e7a-r6.cu        (R6)   FAUTIF
e7a-r8.cu        (R8)   FAUTIF
e7a-r9a.cu       (R9A)  CORRIGE
```

```
FAULTY_GENERATOR_PRESENT_IN=e7a-scaling,e7a-r2,e7a-r3,e7a-r4,e7a-r5,e7a-r6,e7a-r8
```

## 3 — Ce qui tombe, et ce qui tient

### Ce qui tombe — toute la narration « divergence numérique »

| proposition | statut |
|---|---|
| `R6_P48_VS_K_FULL_OUTPUT_MISMATCH=1875` | **RETIRE** — artefact de données |
| `DIVERGENCE_IS_ON_INTEGER_PARTIAL_SUMS` (R7) | **RETIRE** (déjà retiré en R7B) |
| `FLOAT_REASSOCIATION_HYPOTHESIS` (R7/R7B/R8) | **RETIRE** — il n'y a pas de divergence à expliquer |
| `NUMERICAL_DIVERGENCE_ROOT_CAUSE=...` | **RETIRE** |
| `KERNEL_NUMERICAL_RESULT_SENSITIVE_TO_CODEGEN_PERTURBATION=STRONGLY_SUPPORTED` | **RETIRE** |
| `ZERO_ROOT_ARTIFACT_IS_NOT_THE_PRIMARY_NUMERICAL_CAUSE` | **RETIRE** |
| `R8_NATIVE_EQUIVALENCE_GATE=FAIL` | **RETIRE** → le gate **PASSE** |
| `HARNESS_DEFECT_37` | **RESOLU** — mais il n'y avait pas de défaut de noyau |

**R7, R7B et R8 étaient une forensique sur un mismatch fabriqué.** Leurs mesures SASS sont exactes
(les compteurs sont ce qu'ils sont), mais leur objet — « pourquoi les sorties diffèrent » — n'existe
pas. Je n'aurais pas dû raisonner sur un mismatch sans d'abord vérifier la finitude des sorties.

### Ce qui tient

**1. Les mesures de temps de R2-R3 sont valides** pour leurs contrastes appariés. Argument : les
`NaN` sont **identiques dans tous les bras** (mêmes poids, même bug de génération), donc tous les bras
font exactement le même travail arithmétique. Un `NaN` ne change ni le nombre d'`IDP.4A`, ni les
branches, ni les accès mémoire. Les contrastes A100−A0 et même-cardinalité restent donc
interprétables — ils reposent sur une comparaison entre bras, pas sur la valeur des sorties.

Réserve honnête : je **recommande de revérifier** la campagne R2 sur données saines avant de la
considérer comme définitive, parce que « les NaN sont identiques partout » est un argument, pas une
mesure.

**2. Toute l'analyse structurelle SASS reste valable** — c'est du désassemblage, pas du calcul :

```
ADDRESS_AND_LOAD_MASK_IDENTICAL=YES
IDP_COUNT_IDENTICAL=YES (32=32)
NATIVE_IDP_CHAIN_COUNT=19   R8_IDP_CHAIN_COUNT=24
NATIVE_MAX_IDP_CHAIN_LENGTH=4   R6=7   R8=4
R6_I2F_COUNT=3 vs natif 1 ; R8_I2F_COUNT=1
BSSY/BSYNC : natif 2, R6 6, R8 4
```

Ces faits restent vrais et utiles. Ce qui tombe, c'est **l'inférence** que je leur avais accrochée.

**3. Le retrait de l'artefact était une vraie amélioration**, mesurée au SASS :

```
ARTEFACT_RETIRE=span_count_Et_done_compteur
EXTRA_ZERO_ROOT_COUNT=0
REMOVING_ZERO_GUARD_RESTORES=I2F,UI2F,FMUL,FFMA,MAX_INTEGER_CHAIN_LENGTH -> structure native
```

**4. Les réconciliations de R3 restent valides** : elles comparent des temps, pas des sorties.

**5. La leçon du §31/§32** reste bonne méthode, mais elle perd sa preuve empirique : le garde à
valeur nulle changeait bien la structure du code, mais je n'ai **plus de mesure** montrant que cela
changeait un résultat. Je la conserve comme observation structurelle, pas comme démonstration.

## 4 — L'état réel du banc R8

Puisque le retest propre donne `MISMATCH=0`, les gates du §17 de R8 sont en fait franchis :

```
R8_NATIVE_EQUIVALENCE_GATE=PASS                    (mesure propre, R9A)
BITWISE_KERNEL_OUTPUT_MATCH=YES
R8_STRUCTURE_GATE=PASS
ONE_NATIVE_LOOP_BODY_IN_SASS=YES   LOOP_BACKEDGE_PRESENT=YES   FULL_UNROLL=NO
LOOP_CONTROL_ARITHMETIC_INDEPENDENT=YES
EXTRA_ZERO_ROOT_COUNT=0
```

Conformément au §19 de R8 : si structure **et** bitwise passent, `KBX_MULTIPLICITY_TIMING_ALLOWED`
devient `YES`. C'est désormais le cas, sous un défaut de harnais près : mes campagnes R5 à R8 ont
tourné sur données contaminées, donc **avant toute nouvelle mesure**, il faut régénérer avec des
échelles saines.

## 5 — Ce qu'il faut faire maintenant, dans l'ordre

```
1. Corriger le generateur dans TOUS les microbenchs (echelles half finies) — FAIT pour R9A.
2. Re-executer la campagne R2 (famille A) sur donnees saines pour confirmer le contraste A100-A0
   et le gate meme-cardinalite. C'est le resultat scientifique le plus lourd qui repose sur des
   donnees contaminees.
3. Reprendre R6/R8 sur donnees saines : le banc bitwise etant equivalent, la campagne de
   multiplicite kbx (P1/P3/P6/P48) devient enfin executable.
4. HARNESS_DEFECT_38 (table d armes auto-verifiee) avant tout timing.
```

```
RELAXED_EPSILON_TIMING=CLOSED_SANS_OBJET
   -> il n y a plus d ecart numerique a tolerer : les noyaux sont bit a bit identiques.
R6_TIMING_ALLOWED=YES_SOUS_RESERVE_DE_DONNEES_SAINES
KBX_MULTIPLICITY_TIMING_ALLOWED=YES_SOUS_RESERVE_DE_DONNEES_SAINES
```

## 6 — Ce que je dois reconnaître

J'ai construit cinq passes de forensique (R4 à R8) sur une comparaison `a[i] != b[i]` **sans jamais
vérifier que les sorties étaient finies**. Le signal `MAX_ABS_OUTPUT_ERROR=0` accompagné de 1875
mismatches était une contradiction logique que le brief a correctement relevée — et je ne l'ai pas
vue moi-même en R7, en R7B, ni en R8. J'ai même écrit en R7 que « toutes les sorties diffèrent » sans
me demander si elles étaient seulement des nombres.

Les mesures SASS que j'ai produites sont exactes. Les conclusions causales que j'en ai tirées ne le
sont pas. Cette passe les retire.

```
HARNESS_DEFECT_40=FAULTY_WEIGHT_DATA_GENERATOR_NONFINITE_SCALE   STATUS=RESOLVED_BY_FIX
   -> un generateur de donnees qui remplit des octets arbitraires dans un champ de demi-flottant
      produit des NaN silencieux, qui se propagent a toute la ligne de sortie et font passer une
      egalite bit a bit pour une divergence totale.
DETECTION_METHOD_DEFECT=OUTPUT_FINITENESS_NOT_CHECKED_BEFORE_BITWISE_COMPARISON   STATUS=RECORDED
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 7 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r9a/e7a-r9a.cu + bin/e7a-r9a
   Les deux noyaux sont dans UN binaire, meme compilation, meme session.
AUCUN TIMING EXECUTE DANS CETTE PASSE
```

## 8 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R2_STATUS=CLOSED_CONCLUSIVE_NONLINEAR_SITE_DEPENDENT_RESULT_RECOMMANDEE_A_REVERIFIER
ENGINE007A_R3_STATUS=CLOSED_ABSOLUTE_RECONCILIATION_CASE_C_TIMINGS_ONLY
ENGINE007A_R4_STATUS=CLOSED_FACTORIAL_INVALIDATED_BY_KBX_LOOP_HOISTING
ENGINE007A_R5_STATUS=CLOSED_STRUCTURE_GATE_FAILED_ARTIFICIAL_SERIAL_MACHINE
ENGINE007A_R6_STATUS=CLOSED_STRUCTURE_PASS_FIDELITY_FAIL_RETRACTED
ENGINE007A_R7_STATUS=CLOSED_FORENSIC_ON_A_FABRICATED_MISMATCH
ENGINE007A_R7B_STATUS=CLOSED_FORENSIC_ON_A_FABRICATED_MISMATCH
ENGINE007A_R8_STATUS=CLOSED_ARTIFACT_REMOVED_GATE_ACTUALLY_PASSES
ENGINE007A_R9A_STATUS=CLOSED_MISMATCH_WAS_INSTRUMENTATION_DATA_DEFECT
MISMATCH_TOTAL=0
SIGNED_ZERO_MISMATCH_COUNT=0
FINITE_NONZERO_MISMATCH_COUNT=0
NAN_PAYLOAD_MISMATCH_COUNT=0
NAN_VS_NUMBER_COUNT=0
INF_MISMATCH_COUNT=0
OTHER_SPECIAL_COUNT=0
MISMATCH_CLASSIFICATION_ACCOUNTING_MATCH=YES
NATIVE_NAN_COUNT_CLEAN_INPUTS=0
R8_NAN_COUNT_CLEAN_INPUTS=0
NATIVE_NAN_COUNT_FAULTY_INPUTS=1875
R8_NAN_COUNT_FAULTY_INPUTS=1875
R6_WITH_ARTIFACT_MISMATCH_CLEAN_INPUTS=0
BITWISE_KERNEL_OUTPUT_MATCH=YES
R8_NATIVE_EQUIVALENCE_GATE=PASS
MAX_ABS_OUTPUT_ERROR=0_EXACT
MAX_ABS_OUTPUT_ERROR_CAUSE=NO_DIFFERENCE_AT_ALL_NOT_REPORTING_PRECISION
NONFINITE_OUTPUTS_ARE_INPUT_INDUCED_NOT_KERNEL_INDUCED=YES
FAULTY_GENERATOR_PRESENT_IN=e7a-scaling,e7a-r2,e7a-r3,e7a-r4,e7a-r5,e7a-r6,e7a-r8
FAULTY_GENERATOR_FIXED_IN=e7a-r9a
RETRACTED=R7_AND_R7B_AND_R8_NUMERICAL_DIVERGENCE_NARRATIVE
RETAINED=R2_R3_TIMINGS,R3_RECONCILIATION,ALL_SASS_STRUCTURE_MEASUREMENTS
R2_TIMING_VALIDITY=ARGUED_NAN_IDENTICAL_ACROSS_ARMS_RECOMMENDED_TO_REVERIFY
SASS_STRUCTURE_FINDINGS_REMAIN_VALID=YES
REMOVING_ZERO_GUARD_RESTORES_NATIVE_STRUCTURE=YES
NUMERICALLY_ZERO_INSTRUMENTATION_CAN_BE_CODEGEN_ACTIVE=OBSERVED_STRUCTURALLY
   -> la preuve que cela changeait un RESULTAT numerique est retiree
R6_TIMING_ALLOWED=YES_SOUS_RESERVE_DE_DONNEES_SAINES
KBX_MULTIPLICITY_TIMING_ALLOWED=YES_SOUS_RESERVE_DE_DONNEES_SAINES
RELAXED_EPSILON_TIMING=CLOSED_SANS_OBJET
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=READY_FOR_VALID_TEST
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
HARNESS_DEFECT_37=RESOLVED_NO_KERNEL_DEFECT_EXISTED
HARNESS_DEFECT_38=OPEN_ARM_TABLE
HARNESS_DEFECT_40=RESOLVED_BY_GENERATOR_FIX
ARM_TABLE_SHA256=NOT_PRODUCED
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
