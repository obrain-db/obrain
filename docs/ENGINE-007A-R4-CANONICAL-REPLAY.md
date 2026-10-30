# ENGINE-007A-R4 — REPLAY CANONIQUE + DÉCOMPOSITION FACTORIELLE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r4/`

---

## 0 — Résultat

**Le replay canonique réussit. La décomposition factorielle échoue**, et son échec est mesurable.

```
A. REPLAY CANONIQUE (5 processus, 490 observations)
   K_STREAM canon =  8,3344 us   (historique 8,28   ; ecart +0,05)
   K_DECODE canon = 22,8950 us   (historique 22,60  ; ecart +0,30)
   K_FULL   canon = 83,0176 us   (historique 79,94  ; ecart +3,08)

B. FACTORIEL 2x2
   ACTIVATION_LOAD_EFFECT_US      = +0,5419 us
   LARGE_OUTPUT_WRITE_EFFECT_US   = +0,5022 us
   ACTIVATION_X_WRITE_INTERACTION = -0,8075 us

C. LE FAIT DOMINANT
   K_FULL - D11 = 58,09 us    (ce que les bras D ne contiennent PAS)
   D11 - K_DECODE = 2,03 us   (ce que les deux facteurs ajoutent AU TOTAL)
```

Les deux facteurs testés expliquent **2,03 µs sur un écart de 60 µs**. La décomposition ne ferme pas
le compte, et le §21 s'applique :

```
MAJOR_A0_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
```

---

## 1 — Freeze R3 (§1 à §3)

```
ENGINE007A_R3_STATUS=CLOSED_ABSOLUTE_RECONCILIATION_CASE_C
ABSOLUTE_LEVEL_DISAGREEMENT_ROOT_CAUSE=KERNEL_X_HARNESS_INTERACTION
HARNESS_EFFECT_US=0.48_TO_2.38     KERNEL_EFFECT_US=6.65_TO_8.55     INTERACTION_US=-1.90
ENGINE006F_K_FULL_HISTORICAL_REPRODUCIBILITY=CONFIRMED_TO_0.01US
006F_TIMING_RESULT_WAS_REAL=YES
ALL_006F_AND_007A_KERNELS_ARE_ABSOLUTELY_COMPARABLE=YES=NON_PROMU
006F_K_DECODE_EQUIVALENT_TO_007A_A0=NO
   (activations 0 vs 32 ; sortie 2 621 440 vs 20 480 octets)
OLD_006F_DECODE_VS_007A_A0_DIRECT_COST_COMPARISON=INVALID
OUTPUT_WRITE_CONTRIBUTION_US=NOT_MEASURED      Q8_1_ACTIVATION_LOAD_CONTRIBUTION_US=NOT_MEASURED
LARGE_OUTPUT_WRITE_VOLUME=STRONG_CANDIDATE
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS
```

## 2 — Replay canonique (§4, §5, §6) : PASS

Protocole canonique appliqué : `K=2048`, warmup 3 batches, **une paire d'événements par batch
entier**, 5 processus séquentiels à contexte neuf, rotation latine 7 bras × 2 sens, 14 cycles.

| noyau | R4 canonique | historique 006F | écart |
|---|---|---|---|
| `K_STREAM` | **8,3344** (sd 0,146) | 8,28 | +0,054 |
| `K_DECODE` | **22,8950** (sd 0,155) | 22,60 | +0,295 |
| `K_FULL` | **83,0176** (sd 0,438) | 79,94 | +3,078 |

Les trois noyaux sont **repris à l'identique** (mêmes corps) et se reproduisent sous le protocole
canonique. Le replay fournit donc une référence homogène `006F_FAMILY_UNDER_CANONICAL_PROTOCOL`,
**conservée à côté** des valeurs historiques, sans écraser les unes par les autres (§5).

Réserve sur `K_FULL` : l'écart de +3,08 µs est du même ordre que l'effet harnais mesuré en R3
(0,48 à 2,38 µs) plus l'interaction. Le gate du §6 est franchi « dans la résolution
inter-processus » pour `K_STREAM` et `K_DECODE` ; pour `K_FULL` il est franchi à 3,1 µs près, ce que
je déclare plutôt que de le lisser.

**Contrôle croisé qui compte** : R3 mesurait `k_full` sous protocole canonique à **82,32 µs** ; R4
donne **83,02 µs**. Deux instruments, deux binaires, écart de **0,70 µs (0,8 %)**.

## 3 — Le factoriel, et pourquoi il échoue (§7 à §18)

Quatre bras compilés depuis un même corps decode, activations OFF/ON × écriture SMALL/LARGE :

| bras | temps (µs) | sd | `LDG.E.U8` (poids) | `LDG.E.S8` (activations) | registres |
|---|---|---|---|---|---|
| D00 = A off, W small | 23,8852 | 0,543 | 8 | **0** | 21 |
| D10 = A on, W small | 24,8308 | 0,179 | 8 | **8** | 34 |
| D01 = A off, W large | 24,7912 | 0,138 | 8 | **0** | 22 |
| D11 = A on, W large | 24,9293 | 0,153 | 8 | **8** | 34 |

Le gate SASS du §9 est franchi (`Q8_LDG` : 0 pour A=off, 8 statiques pour A=on, identique entre D10
et D11) et le corps decode est constant : `LDG.E.U8=8`, `LOP3=65`, `SHF=26` dans les **quatre** bras.

Estimations demandées (§16 à §18) :

```
ACT_EFFECT_SMALL = D10 - D00 = +0,9456 us (sd 0,5724)
ACT_EFFECT_LARGE = D11 - D01 = +0,1381 us (sd 0,2439)
ACTIVATION_LOAD_EFFECT_US = +0,5419 us

WRITE_EFFECT_NO_ACT   = D01 - D00 = +0,9060 us (sd 0,5483)
WRITE_EFFECT_WITH_ACT = D11 - D10 = +0,0985 us (sd 0,2540)
LARGE_OUTPUT_WRITE_EFFECT_US = +0,5022 us

ACTIVATION_X_WRITE_INTERACTION_US = -0,8075 us
```

**Aucun des deux facteurs n'est important, et l'interaction est du même ordre que les effets
principaux.** Le §22 s'applique : *« ne pas publier une décomposition additive »*.

```
ACT_X_WRITE_INTERACTION=STRONG_RELATIVE_TO_MAIN_EFFECTS
ADDITIVE_DECOMPOSITION=NOT_PUBLISHED
LARGE_OUTPUT_WRITE_CONTRIBUTION_US=+0,50_SMALL
Q8_1_ACTIVATION_LOAD_CONTRIBUTION_US=+0,54_SMALL
```

## 4 — Le défaut qui explique tout : la boucle kbx est hissée

Le comptage des chargements dans le SASS des bras D révèle la cause :

```
k_full   : 65 LDG statiques  (dont 32 U8 poids + 32 S8 activations)
k_decode :  8 LDG statiques  (8 U8 poids)
D00/D01  :  8 LDG statiques  (8 U8 poids)
D10/D11  : 16 LDG statiques  (8 U8 poids + 8 S8 activations)
```

Un thread parcourt **12 itérations** de la boucle `kbx`. Le noyau complet exécute donc
`12 x 8 = 96` chargements de poids par thread, mais les bras D n'en font que **8** : le compilateur a
**hissé les 12 itérations hors de la boucle**. Le nombre d'instructions `LDG` statiques est identique
entre D00 et D01, et les activations ne sont chargées qu'une fois au lieu de douze.

Le corps decode des bras D n'est donc **pas** celui de `k_decode` : c'est une version **12 fois plus
légère** en chargements. C'est visible dans les temps : D00 = 23,89 µs contre `K_DECODE` = 22,90 µs,
soit **+1,0 µs** — les bras D ne font presque rien de plus que le décodage nu, alors qu'ils portent
en plus les activations et l'écriture large.

```
D_ARMS_KBX_LOOP_HOISTED=YES
D_ARMS_DYNAMIC_LOAD_COUNT=8_AU_LIEU_DE_96
FACTORIAL_WORKLOAD_STRUCTURE_GATE=FAIL
FACTORIAL_DESIGN_INVALID_FOR_THE_INTENDED_QUESTION
```

**Conséquence honnête** : les chiffres du §3 sont **internes** à la famille D et nuls pour la
question posée. Ce qu'ils montrent est limité mais réel : quand les chargements par itération
disparaissent du chemin chaud, ni les activations (+0,54 µs) ni l'écriture large (+0,50 µs) ne
coûtent quoi que ce soit. Ils **ne disent rien** sur le coût de ces deux facteurs dans `k_full`, où
les chargements sont réellement répétés 12 fois.

## 5 — Le fait dominant, chiffré

```
K_FULL        = 83,02 us
D11           = 24,93 us   (decodage + activations + ecriture large, boucle hissee)
K_FULL - D11  = 58,09 us   NON EXPLIQUE par les deux facteurs testes
D11 - K_DECODE=  2,03 us   ce que les deux facteurs ajoutent AU TOTAL
ratio          = 3,33 x
```

Sur les 60 µs qui séparent le décodage nu du noyau complet, la décomposition testée n'en couvre que
**2,03**. Le §21 est le cas applicable :

```
MAJOR_A0_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
006F_K_DECODE_LOW_LATENCY_VS_A0_DIFFERENCE=LARGELY_EXPLAINED_BY_DIFFERENT_OUTPUT_SEMANTICS=NON_PROMU
A0_FIXED_FLOOR_HAS_LARGE_ACTIVATION_PATH_COMPONENT=NO_POUR_LES_FACTEURS_TESTES
```

Ce que 006G/H avaient appelé « l'écart 22,60 vs 90,69 » est donc, au moins pour l'essentiel,
**le coût du chemin de chargement répété que seul le noyau complet possède** — et non la sémantique
d'écriture ni les 32 activations en elles-mêmes. C'est cohérent avec 006G (aucune sensibilité
mémoire, aucune sensibilité au cache) et avec 007A-R2 (retirer les `dp4a` ne rend que 3,4 µs) :
**le noyau complet paie ses 12 passes de chargement, et c'est là que passe le temps.**

Je ne promus pas cette lecture au-delà de ce qu'elle est : c'est la seule hypothèse compatible avec
trois mesures convergentes, mais elle n'est pas elle-même mesurée. Le test propre serait de forcer
la boucle kbx à ne pas être hissée dans les bras D — ce que je n'ai pas fait.

## 6 — Blocages maintenus (§26, §27)

```
ENGINE007B_STATUS=BLOCKED     ENGINE007C_STATUS=BLOCKED
USEFUL_DP4A_WORK_IS_ON_THE_WALL_CRITICAL_PATH_MAIS_FAIBLE=3.65_PERCENT
IDP_COUNT_ALONE_INSUFFICIENT=YES
   -> aucune transformation de modele reduisant la cardinalite du travail n'a de relation monotone
      suffisante avec le wall pour justifier sa complexite.
AUCUNE_SPARSITE_AUCUN_LOW_RANK_AUCUN_PRUNING
```

R4 ne remet pas en cause les contrastes internes de R2 (§25) :
`A100_MINUS_A0=3,4391 us`, `IDP_COUNT_ALONE_INSUFFICIENT=YES`,
`ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES`.

## 7 — Protocole canonique promu (§28)

```
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS
   BATCH_LENGTH_K=2048            ONE_EVENT_PAIR_PER_BATCH_ENTIER
   WARMUP=3_batches               FRESH_PROCESS_PER_CAMPAIGN (>=5)
   LATIN_ROTATION_AVEC_ALTERNANCE DE SENS
   UNITE_STATISTIQUE=moyenne_par_processus ; CONTRASTES_APPARIES_PAR_CYCLE
   TRAVAIL_NEUTRE_ENTRE_BRAS=non_utilise_en_R4_la_rotation_suffit
```

Mesuré sur cette campagne : `N_OBSERVATIONS=490`, `N_PROCESSES=5`, écart-type inter-processus de
0,14 à 0,54 µs selon le bras.

## 8 — Provenance et défauts

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r4/e7a-r4.cu + bin/e7a-r4
   Les sept noyaux sont dans UN binaire, meme compilation, meme session.
PLUMBING_FAILURE_29=NON_ASSIGNE
```

Défauts de harnais de cette passe :

```
HARNESS_DEFECT_33=VERSION_INITIALE_SANS_PTX_INLINE_ECRITURE_LARGE_REPLIEE
   -> le compilateur reduisait les 32 stores par thread a un store conditionnel unique ; D00 et D01
      avaient un SASS IDENTIQUE (577 lignes, 1 STG) et etaient indiscernables. Corrige par
      st.global.cg PTX inline. Detecte au gate SASS AVANT toute mesure.
HARNESS_DEFECT_34=EXTRACTION_DE_SYMBOLE_MANGLED_TRONQUEE
   -> trois noyaux sur sept ont d'abord ete desassembles en 0 ligne (nom tronque). Non interprete.
HARNESS_DEFECT_35=BOUCLE_KBX_HISSEE_DANS_LES_BRAS_FACTORIELS
   -> les 12 iterations sont hissees : 8 LDG au lieu de 96 par thread. Le corps des bras D n'est
      pas celui de k_decode. C'est la cause de l'echec de la decomposition, et cela reste OUVERT :
      la question du paragraphe 7 exigerait des chargements non hissables.
```

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R2_STATUS=CLOSED_CONCLUSIVE_NONLINEAR_SITE_DEPENDENT_RESULT
ENGINE007A_R3_STATUS=CLOSED_ABSOLUTE_RECONCILIATION_CASE_C
ENGINE007A_R4_STATUS=CLOSED_REPLAY_OK_FACTORIAL_DESIGN_INVALID
006F_K_FULL_HISTORICAL_REPRODUCIBILITY=CONFIRMED
006F_TIMING_RESULT_WAS_REAL=YES
006F_K_DECODE_EQUIVALENT_TO_007A_A0=NO
OUTPUT_WRITE_CONTRIBUTION_US=NOT_MEASURED_FOR_K_FULL
Q8_1_ACTIVATION_LOAD_CONTRIBUTION_US=NOT_MEASURED_FOR_K_FULL
CANONICAL_REPLAY_K_STREAM_US=8.3344
CANONICAL_REPLAY_K_DECODE_US=22.8950
CANONICAL_REPLAY_K_FULL_US=83.0176
CANONICAL_REPLAY_REPRODUCIBILITY=PASS
R3_VS_R4_K_FULL_CANONICAL=82.32_VS_83.02_ECART_0.70US
FACTORIAL_ARMS_D00_D10_D01_D11=23.8852_24.8308_24.7912_24.9293
ACTIVATION_LOAD_EFFECT_US=+0.5419
LARGE_OUTPUT_WRITE_EFFECT_US=+0.5022
ACTIVATION_X_WRITE_INTERACTION_US=-0.8075
ADDITIVE_DECOMPOSITION=NOT_PUBLISHED_INTERACTION_TOO_LARGE
FACTORIAL_WORKLOAD_STRUCTURE_GATE=FAIL
D_ARMS_KBX_LOOP_HOISTED=YES
D_ARMS_DYNAMIC_LOAD_COUNT=8_AU_LIEU_DE_96
D11_MINUS_K_DECODE_US=2.03
K_FULL_MINUS_D11_US=58.09_UNEXPLAINED
MAJOR_A0_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
006F_K_DECODE_LOW_LATENCY_VS_A0_DIFFERENCE=LARGELY_EXPLAINED_BY_DIFFERENT_OUTPUT_SEMANTICS=NON_PROMU
A0_FIXED_FLOOR_HAS_LARGE_ACTIVATION_PATH_COMPONENT=NO_POUR_LES_FACTEURS_TESTES
HYPOTHESE_RESIDUELLE=LE_NOYAU_COMPLET_PAIE_SES_12_PASSES_DE_CHARGEMENT=COMPATIBLE_AVEC_3_MESURES_NON_MESUREE
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS
SAME_KERNEL_HISTORICAL_REPRODUCIBILITY=ESTABLISHED_FOR_K_FULL
CROSS_KERNEL_ABSOLUTE_TIMING_COMPARABILITY=REQUIRES_EXPLICIT_WORKLOAD_EQUIVALENCE
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
NEXT_STEP_IF_PURSUED=BRAS_FACTORIELS_AVEC_CHARGEMENTS_NON_HISSABLES
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
