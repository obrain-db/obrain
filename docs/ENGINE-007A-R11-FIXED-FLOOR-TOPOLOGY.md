# ENGINE-007A-R11 — PLANCHER FIXE ET SEUIL TOPOLOGIQUE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r11/`

---

## 0 — Résultat

**Il n'y a pas de plancher fixe.** Le `KBX00_EMPTY` — le noyau complet, même grille, même bloc,
même binaire, avec un masque vide — coûte **6,21 µs**. Et **six blocs sur quarante-huit** suffisent à
faire monter le noyau à **82,04 µs**.

```
KBX00_EMPTY   =  6,2103 us      (aucun corps kbx execute, tout le reste identique)
KBX06_SET_A   = 82,0382 us      (6 blocs = 12,5 % du travail)
KBX12_SET_A   = 82,0196 us
KBX24_SET_A   = 82,2568 us
KBX36_SET_A   = 83,8492 us
KBX48_NATIVE  = 83,9440 us
K_FULL_NATIVE = 84,1660 us

FIXED_SCAFFOLD_COMPONENT      = 6,21 us   (7,4 % du plein)
NONZERO_BODY_ACTIVATION       = +75,83 us (KBX06 - KBX00)
HIGH_REGION_THRESHOLD         = +1,91 us  (KBX48 - KBX06)

ENGINE007A_R11_STATUS=CLOSED_NONZERO_BODY_ACTIVATION_THRESHOLD
CAS_B_DU_BRIEF=APPLIQUE
```

Le coût n'est pas proportionnel au travail : il est **déclenché par le fait d'exécuter un corps**,
pas par la quantité de corps. `KBX06` fait **12,5 %** du travail de `KBX48` et coûte **97,7 %** de
son temps.

---

## 1 — Freeze R10 (§1 à §5)

```
ENGINE007A_R10_STATUS=CLOSED_KBX_COUNT_NOT_DOMINANT
K_FULL_NATIVE_US=84.3000   KBX48_NATIVE_US=84.0828
FULL_RUNTIME_MINUS_NATIVE_US=-0.2173   CI95=[-0.3269,-0.1077]   RATIO=0.99742
FULL_RUNTIME_STATISTICALLY_IDENTICAL_TO_NATIVE=NO
RUNTIME_LOOP_TIMING_EQUIVALENCE=PASS_BY_PREDECLARED_MATERIALITY_RESOLUTION

KBX12_RANGE_US=0.1060            KBX12_PLACEMENT_EFFECT=NOT_MATERIALLY_DETECTED
KBX24_A_MINUS_B_US=+0.1021       KBX24_PLACEMENT_EFFECT=DETECTABLE_BUT_IMMATERIAL
KBX06_A_MINUS_B_US=-0.0915       KBX06_PLACEMENT_EFFECT=NOT_MATERIAL
KBX_PLACEMENT_EFFECT_IN_QUALIFIED_CONTROLS=NOT_MATERIAL
KBX_PLACEMENT_NEVER_MATTERS=NON_PROMU

KBX_DYNAMIC_BODY_MULTIPLICITY=NOT_DOMINANT
KBX_COUNT_AS_PRIMARY_WALL_VARIABLE=REJECTED
R4_FULL_FLOOR_EXPLANATION_BY_KBX_COUNT=FALSIFIED
LINEAR_KBX_COUNT_MODEL=NOT_ALLOWED
DISCRETE_WALL_TRANSITION_BETWEEN_24_AND_36_KBX=MEASURED
   -> CACHE/OCCUPANCY/WARP/MEMORY/SCHEDULER_THRESHOLD restent des hypotheses NON promues.

NEITHER_IDP_CARDINALITY_NOR_KBX_CARDINALITY_DOMINATES_WALL=SUPPORTED_CLEAN_DATA
   (formulation du paragraphe 7 : aucun composant n'est declare pouvoir couter plus que le tout,
    les contributions n'etant pas additives.)
```

## 2 — Phase A : descripteurs topologiques (§10 à §12)

Dérivés des données de qualification R10 gelées, sur les 128 threads du mapping natif.

| arme | blocs | dyn | T0 | T1 | T2 | AWARP | AEXEC | iter0 | iter1 |
|---|---|---|---|---|---|---|---|---|---|
| `KBX00_EMPTY` | 0 | 0 | 128 | 0 | 0 | 0/0/0/0 | 0/0/0/0 | 0 | 0 |
| `KBX06_SET_A` | 6 | 24 | 112 | 8 | 8 | 4/4/4/4 | 8/8/4/4 | 16 | 8 |
| `KBX12_SET_A` | 12 | 48 | 96 | 16 | 16 | 8/8/8/8 | 16/16/8/8 | 32 | 16 |
| `KBX24_SET_A` | 24 | 96 | **64** | 32 | 32 | 16/16/16/16 | 32/32/16/16 | 64 | 32 |
| `KBX36_SET_A` | 45 | 180 | **0** | 76 | 52 | **32/32/32/32** | 56/60/32/32 | 116 | 64 |
| `KBX48_NATIVE` | 48 | 192 | 0 | 64 | 64 | 32/32/32/32 | 64/64/32/32 | 128 | 64 |

(T0/T1/T2 = threads avec 0 / 1 / 2 corps sélectionnés ; AWARP = threads actifs par warp ;
AEXEC = exécutions de corps par warp.)

**Le filtre du §12** exige un descripteur qui change entre 24 et 36 **et** peu ou pas entre 36 et 48.
Un seul le satisfait pleinement :

```
AWARP_24=16/16/16/16  ->  AWARP_36=32/32/32/32   delta=+16 par warp   (change)
AWARP_36=32/32/32/32  ->  AWARP_48=32/32/32/32   delta=0              (ne change pas)

T0_24=64  ->  T0_36=0      (change)   T0_36=0 -> T0_48=0   (ne change pas)

CANDIDAT_UNIQUE=TOUS_LES_THREADS_DEVIENNENT_ACTIFS
   -> a 24 blocs, 64 threads sur 128 ne font aucun travail de corps.
   -> a 36 blocs, les 128 threads en font tous.
```

(AEXEC change aussi, mais continue de changer entre 36 et 48 : +12 ; T2 change de +20 puis +12.
AWARP et T0 sont les seuls à suivre exactement la forme temporelle.)

```
IS_THERE_A_DISCRETE_TOPOLOGY_FEATURE_ABSENT_AT_24_AND_PRESENT_AT_36? = YES
   ALL_THREADS_ACTIVE : absent a 24, present a 36.
CAUSALITE=NON_INFERE
```

## 3 — Phase B : le bras à travail nul (§13 à §17)

`KBX00_EMPTY` ajouté à une **table R11 neuve** — la table R10 n'a pas été touchée (§15) :

```
R10_ARM_TABLE_SHA256=e71b8358c80333f7526dc55c40bfcd5a92dd20fe99cfb1b6ba652bcd470a5ba7  (immuable)
R11_ARM_TABLE_SHA256=e6270461582bef9b8ea4fa6b9ace37c795e31c755d2c5c6d935e3fbf6f733abe
```

`KBX00_EMPTY` : `MASK=0`, `KBX_COUNT=0`, `EXPECTED_DYNAMIC_KBX_COUNT=0`, **le même noyau runtime**,
la même grille, le même bloc, le même binaire. Seul le masque change.

Qualification avant chronométrage :

```
HOTE      : EXPECTED_KBX_SET={}   EXPECTED_DYNAMIC_KBX_COUNT=0   HOST_SEQUENCE_MATCH=YES
GPU       : GPU_OBSERVED_KBX_SET=  GPU_OBSERVED_DYNAMIC_KBX_COUNT=0
            SET_MATCH=YES  COUNT_MATCH=YES  PERBLOCK_MATCH=YES
GPU_KBX_QUALIFICATION=PASS
```

Sanité numérique (§17) :

```
OUTPUT_NAN_COUNT=0   OUTPUT_INF_COUNT=0
SEMANTIQUE_ATTENDUE=reduction et store d un accumulateur nul (la boucle ne s execute pas)
BITWISE_MATCH_TO_K_FULL=NON_APPLIQUE   (condition de diagnostic, pas de calcul modele)
```

## 4 — Chronométrage (§18 à §20)

Protocole canonique appliqué : `K=2048`, warmup 3, une paire d'événements par lot, 5 processus
séquentiels à contexte neuf, rotation latine sur 7 bras × 2 sens, 14 cycles, **490 observations**.

| bras | moyenne | sd |
|---|---|---|
| `KBX00_EMPTY` | **6,2103** | 0,1207 |
| `KBX06_SET_A` | **82,0382** | 0,4505 |
| `KBX12_SET_A` | **82,0196** | 0,3156 |
| `KBX24_SET_A` | **82,2568** | 0,2120 |
| `KBX36_SET_A` | **83,8492** | 1,3765 |
| `KBX48_NATIVE` | **83,9440** | 0,6602 |
| `K_FULL_NATIVE` | **84,1660** | 0,6876 |

Moyennes par processus :

| pid | KBX00 | KBX06 | KBX12 | KBX24 | KBX36 | KBX48 | K_FULL |
|---|---|---|---|---|---|---|---|
| 0 | 6,191 | 81,410 | 81,679 | 82,129 | 82,320 | 82,897 | 83,386 |
| 1 | 6,175 | 82,198 | 82,114 | 82,178 | 83,420 | 83,862 | 83,854 |
| 2 | 6,180 | 82,190 | 82,103 | 82,289 | 83,919 | 84,188 | 84,237 |
| 3 | 6,205 | 82,182 | 82,101 | 82,346 | 84,485 | 84,257 | 84,607 |
| 4 | 6,301 | 82,211 | 82,101 | 82,342 | 85,102 | 84,516 | 84,745 |

```
MEAN_US_BY_POSITION_RANGE=0.1475 us   -> TIMING_CAMPAIGN_GATE=PASS
ARM_TABLE_SHA256_POST_RUN=identique    -> R11_TIMING_RESULT=VALID
OUTPUT_SANITY_GATE=PASS (7/7 bras)     INPUT_SANITY_GATE=PASS (5/5 processus)
```

Contrastes appariés vs `KBX00` (§20) :

```
KBX06 - KBX00 = +75,8279   IC95=[+75,7237 ; +75,9320]
KBX12 - KBX00 = +75,8093   IC95=[+75,7288 ; +75,8898]
KBX24 - KBX00 = +76,0465   IC95=[+75,9850 ; +76,1079]
KBX36 - KBX00 = +77,6389   IC95=[+77,3291 ; +77,9486]
KBX48 - KBX00 = +77,7337   IC95=[+77,5818 ; +77,8856]
K_FULL- KBX00 = +77,9556   IC95=[+77,8020 ; +78,1093]
```

## 5 — Classification : `Case B` (§22)

```
KBX00_US=6,2103     KBX06_US=82,0382     ratio KBX00/KBX06 = 0,0757

NONZERO_KBX_ACTIVATION_THRESHOLD=MEASURED
FIXED_SCAFFOLD_COMPONENT_US=6,2103
NONZERO_BODY_ACTIVATION_COMPONENT_US=+75,8279
HIGH_REGION_THRESHOLD_COMPONENT_US=+1,9058
```

Le §22 s'applique : `KBX00 << KBX06 ≈ 82 µs`. L'effet dominant est **déclenché par l'exécution
d'un peu de travail de corps**, pas mis à l'échelle par sa quantité.

Décomposition empirique des trois composantes (§23) — ce sont des contrastes, **pas des mécanismes
supposés additifs** :

| composante | valeur | part |
|---|---|---|
| échafaudage fixe (`KBX00`) | 6,21 µs | 7,4 % |
| activation d'un travail de corps non nul (`KBX06 − KBX00`) | 75,83 µs | 90,1 % |
| région haute cardinalité (`KBX48 − KBX06`) | 1,91 µs | 2,3 % |

## 6 — Ce que cela établit, et ce que cela n'établit pas

**Établi :**

```
KBX00_APPROXIMATELY_EQUALS_KBX06=NO   -> le cas A du paragraphe 21 NE s'applique PAS
MAJOR_FIXED_FLOOR_EXISTS_WITH_ZERO_SELECTED_KBX_BODY=REJECTED
SELECTED_KBX_BODY_EXECUTION_IS_REQUIRED_FOR_THE_DOMINANT_COST=MEASURED
LAUNCH_OR_SCAFFOLD_COST_IS_ONLY_7,4_PERCENT_OF_THE_KERNEL=MEASURED
LE_COUT_EST_DECLENCHE_ET_NON_PROPORTIONNEL=MEASURED
   (12,5 % du travail donne 97,7 % du temps)
```

Cela ferme la question ouverte depuis R4 d'une manière inattendue : le « plancher fixe » de ~82 µs
n'est **pas** fixe. Il exige du travail de corps, mais une quantité infime suffit. Le plancher est
un **seuil d'activation**, pas une constante.

**Non établi, et je ne le promus pas :**

```
MECANISME=INCONNU
   -> latence memoire, chaine de dependance, remplissage de pipeline, cout de vague,
      serialisation d une seconde execution de corps, tout cela reste possible.
LAUNCH_BOUND / MEMORY_BOUND / COMPUTE_BOUND / ISSUE_BOUND / LATENCY_BOUND=NON_PROMUS
   (paragraphe 26 respecte)
CACHE/OCCUPANCY/WARP/MEMORY/SCHEDULER_THRESHOLD=NON_PROMUS
```

## 7 — Le seuil 24 → 36 : l'hypothèse la plus économique, mais non testée

Le descripteur `AWARP`/`T0` est le seul à suivre la forme temporelle (§2). L'hypothèse qui en
découle, et qu'il faut considérer comme **une hypothèse** :

> Avant 36 blocs, une partie des threads ne fait aucun travail de corps ; le coût reste plat.
> À partir de 36, **les 128 threads** exécutent au moins un corps, et le temps fait un pas de
> +1,9 µs. Le plateau 36 → 48 suivrait parce que le nombre de threads faisant **deux** corps ne
> passe que de 52 à 64.

Cette lecture explique aussi pourquoi la région 6 → 24 est parfaitement plate : à 6 blocs comme à
24, les threads actifs font **un ou deux** corps et le temps ne bouge pas — ce qui est cohérent avec
une latence dominée par le **démarrage** du travail de corps plutôt que par sa répétition.

**Le test causal reste à faire** (§24) : deux masques de même cardinalité logique, même cardinalité
dynamique, même travail par warp, ne différant que par « tous les threads actifs ». Le processus de
table d'armes est en place pour cela.

Je ne promus pas cette hypothèse : elle est compatible avec trois observations, elle n'est pas
mesurée.

## 8 — Conséquences pour le programme

```
ENGINE007_CARDINALITY_LEVERS_TESTED=IDP_SITE_COUNT,NATIVE_KBX_BODY_COUNT
BOTH_NOT_DOMINANT=YES
ANOTHER_LOCAL_CARDINALITY_REDUCTION_EXPERIMENT=NOT_JUSTIFIED_WITHOUT_NEW_CAUSAL_EVIDENCE

IDP_CARDINALITY_EFFECT=3,22_PERCENT      (R2-SANE)
KBX_CARDINALITY_EFFECT=2,54_PERCENT      (R10)
NE_PAS_ADDITIONNER_CES_POURCENTAGES=YES  (paragraphe 29)

ENGINE007B_STATUS=BLOCKED      ENGINE007C_STATUS=BLOCKED
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
```

La réduction de cardinalité — le seul axe de réduction de travail identifié jusqu'ici — est
désormais **sans objet** : supprimer du travail ne rend rien, parce que le coût n'est pas gouverné
par la quantité de travail. Toute piste de type sparsité ou rang faible reposait implicitement sur
cette proportionnalité, qui n'existe pas.

## 9 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r11/e7a-r11t.cu + bin/e7a-r11t
   SEPT bras, DEUX noyaux seulement (natif et runtime a masque) ; tous les bras a masque
   partagent le meme code. CODEGEN_CONSTANT_ACROSS_PARTIAL_ARMS=YES
   ADDITIONAL_KEEPALIVE=NONE   ANTI_HOIST_EXPRESSION=NONE   ZERO_VALUED_GUARD=NONE
GENERATEUR=SAIN_QUALIFIE_R9A (echelles fp16 finies normales non nulles)
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS
AUCUN REGLAGE MODIFIE EN COURS DE CAMPAGNE
AUCUNE CAMPAGNE PERDUE : une premiere execution a fait tourner 12 bras par erreur (le drapeau
   de restriction n etait pas applique) ; ses donnees ont ete ecartees et la campagne relancee
   apres correction, avec verification du nombre de bras AVANT execution.
PLUMBING_FAILURE_29=NON_ASSIGNE
```

Défauts de harnais de cette passe :

```
HARNESS_DEFECT_41=R11_TABLE_FILTER_NOT_APPLIED_IN_TIME_MODE
   -> le drapeau R11 ne filtrait la liste d armes que dans le mode de generation de table ; le mode
      de chronometrage a donc mesure 12 bras R10 au lieu des 7 bras R11. Donnees ecartees, cause
      corrigee par une reorganisation de la liste et un harnais R11 dedie. Les 12 bras mesures
      avaient tous des sorties finies et une table coherente : aucune donnee trompeuse publiee.
   STATUS=RESOLVED
HARNESS_DEFECT_42=HOST_SIMULATION_ARTIFACT_ORDER
   -> l artefact de simulation hote produit par script suit l ordre alphabetique des armes, non
      l ordre de la table. Contenu identique, ordre different. Documente, non bloquant.
   STATUS=RECORDED
```

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R10_STATUS=CLOSED_KBX_COUNT_NOT_DOMINANT
ENGINE007A_R11_STATUS=CLOSED_NONZERO_BODY_ACTIVATION_THRESHOLD
R10_ARM_TABLE_SHA256=e71b8358c80333f7526dc55c40bfcd5a92dd20fe99cfb1b6ba652bcd470a5ba7
R11_ARM_TABLE_SHA256=e6270461582bef9b8ea4fa6b9ace37c795e31c755d2c5c6d935e3fbf6f733abe
R11_ARM_TABLE_POST_RUN=IDENTICAL
R11_TIMING_RESULT=VALID
INPUT_SANITY_GATE=PASS
OUTPUT_SANITY_GATE=PASS
N_OBSERVATIONS=490
MEAN_US_BY_POSITION_RANGE=0.1475
TIMING_CAMPAIGN_GATE=PASS
KBX00_US=6.2103
KBX06_US=82.0382
KBX12_US=82.0196
KBX24_US=82.2568
KBX36_US=83.8492
KBX48_US=83.9440
K_FULL_NATIVE_US=84.1660
KBX06_MINUS_KBX00_US=+75.8279
KBX12_MINUS_KBX00_US=+75.8093
KBX24_MINUS_KBX00_US=+76.0465
KBX36_MINUS_KBX00_US=+77.6389
KBX48_MINUS_KBX00_US=+77.7337
FIXED_SCAFFOLD_COMPONENT_US=6.2103
FIXED_SCAFFOLD_FRACTION=7.4_PERCENT
NONZERO_BODY_ACTIVATION_COMPONENT_US=+75.8279
NONZERO_BODY_ACTIVATION_FRACTION=90.1_PERCENT
HIGH_REGION_THRESHOLD_COMPONENT_US=+1.9058
KBX00_OVER_KBX06_RATIO=0.0757
WORK_FRACTION_KBX06_OVER_KBX48=12.5_PERCENT
TIME_FRACTION_KBX06_OVER_KBX48=97.7_PERCENT
NONZERO_KBX_ACTIVATION_THRESHOLD=MEASURED
KBX00_APPROXIMATELY_EQUALS_KBX06=NO
MAJOR_FIXED_FLOOR_EXISTS_WITH_ZERO_SELECTED_KBX_BODY=REJECTED
SELECTED_KBX_BODY_EXECUTION_IS_REQUIRED_FOR_THE_DOMINANT_COST=MEASURED
LAUNCH_OR_SCAFFOLD_COST_IS_ONLY_7.4_PERCENT=MEASURED
COST_IS_TRIGGERED_NOT_PROPORTIONAL=MEASURED
KBX_WORK_QUANTITY_GOVERNS_WALL=REJECTED
CANDIDATE_TOPOLOGY_FEATURE=ALL_THREADS_ACTIVE
ALL_THREADS_ACTIVE_ABSENT_AT_24_PRESENT_AT_36=YES
FEATURE_CHANGES_24_TO_36=YES
FEATURE_CHANGES_36_TO_48=NO
R11_THRESHOLD_CAUSAL_TEST=NOT_YET_RUN
MECANISME_DU_SEUIL=INCONNU
LAUNCH_BOUND=NON_PROMU
MEMORY_BOUND=NON_PROMU
COMPUTE_BOUND=NON_PROMU
ISSUE_BOUND=NON_PROMU
LATENCY_BOUND=NON_PROMU
ENGINE007_CARDINALITY_LEVERS_TESTED=IDP_SITE_COUNT,NATIVE_KBX_BODY_COUNT
BOTH_NOT_DOMINANT=YES
ANOTHER_LOCAL_CARDINALITY_REDUCTION_EXPERIMENT=NOT_JUSTIFIED_WITHOUT_NEW_CAUSAL_EVIDENCE
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=REFORMULATED_AS_ACTIVATION_THRESHOLD
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
HARNESS_DEFECT_41=RESOLVED
HARNESS_DEFECT_42=RECORDED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
