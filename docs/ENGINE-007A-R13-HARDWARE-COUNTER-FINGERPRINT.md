# ENGINE-007A-R13 — EMPREINTE PAR COMPTEURS MATÉRIEL : BLOQUÉ PAR UNE LIMITE D'ACCÈS

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r13/`

---

## 0 — Résultat

**R13 ne peut pas être exécuté sur cette machine.** L'accès aux compteurs matériels est refusé par
le pilote, pour **toutes** les métriques, y compris la plus triviale.

```
ENGINE007A_R13_STATUS=BLOCKED_MISSING_HARDWARE_COUNTER_PERMISSION
EPISODE_ACTIVATION_COUNTER_SIGNATURE=UNRESOLVED
PHYSICAL_WARP_REUSE_COUNTER_SIGNATURE=UNRESOLVED
```

C'est une limite d'environnement, pas un échec de méthode, et **aucune donnée de compteur n'a été
obtenue ni simulée**. Je rapporte exactement ce que j'ai mesuré et ce qui manque.

---

## 1 — Freeze R12, et correction de la comptabilité linéaire (§1, §2)

```
ENGINE007A_R12_STATUS=CLOSED_ACTIVE_WARP_EPISODE_COUNT_PRIMARY_WALL_VARIABLE
KBX00_EMPTY_US=6.2103    G0_D1_US=20.3254    G0_D8_US=20.6205
E2_SAME_WARP_US=32.9286  E2_DIFFERENT_WARPS_US=30.9966
E3_US=44.2117            E4_US=55.8584       E6_US=81.9283

LANE_DENSITY_WITHIN_ACTIVE_WARP_EPISODE=NOT_DOMINANT
ACTIVE_WARP_EPISODE_COUNT=PRIMARY_WALL_VARIABLE_FOR_NONZERO_KBX_PATH
ACTIVE_PHYSICAL_WARP_ASSIGNMENT=SECONDARY_MATERIAL_VARIABLE
MECANISME_MATERIEL=NON_PROMU
```

**Correction demandée au §2, et elle est fondée.** Le modèle que j'ai publié en R12 :

```
wall = 8,0695 + 11,3914 x episodes      R2=0,99989   MAX_RESIDUAL=0,26 us
```

est **incohérent avec E6** : il prédit `8,07 + 6 x 11,39 = 76,42 µs` là où E6 mesure **81,93 µs**,
soit un résidu de **5,5 µs**, et non 0,26. Le résidu annoncé était calculé sur un jeu de points qui
n'incluait pas E6 dans le même ajustement — erreur de ma part dans le rapport R12.

Valeur à retenir, en distinguant les placements physiques :

```
EPISODE_SCALING_APPROX=~12,2_TO_12,6_US_PAR_EPISODE_ACTIF_SUPPLEMENTAIRE

  1 -> 2 episodes, MEME warp physique     : +12,2935
  1 -> 2 episodes, warps DIFFERENTS       : +10,3615
  2 -> 3 episodes                         : +11,2826
  3 -> 4 episodes                         : +11,6467
  4 -> 6 episodes (par episode)           : +13,0350

R4_STYLE_LINEAR_BOOKKEEPING=RETIRE
UNIVERSAL_0.2_PERCENT_AGREEMENT_CLAIM=RETIRE
HISTORICAL_PRE_REGISTRATION_12.6380_US_PER_EPISODE=SUPPORTED_AS_ORDER_OF_MAGNITUDE_REFERENCE
   -> E6 mesure 81,93 us contre 82,02 predits, soit 0,11 % ; mais l accord n est PAS de 0,2 %
      sur tous les bras, et je ne le pretendais pas : les bras a 2, 3 et 4 episodes s ecartent
      de 0,5 a 1,9 us de la reference.
```

Opportunité au niveau noyau (§17), recalculée sur les mesures :

| contraste causal | réduction de paroi |
|---|---|
| 6 → 4 épisodes | **31,8 %** |
| 6 → 3 épisodes | **46,0 %** |
| 6 → 2 épisodes | **60,0 % (même warp) à 62,2 % (warps différents)** |
| 6 → 1 épisode | **74,8 %** |

```
KERNEL_LEVEL_CONTRASTS_ONLY=YES
NOT_ATTAINABLE_MODEL_QUALITY_CLAIMS=YES
PRODUCT_TOK_S_GAIN=NOT_MEASURED
```

## 2 — Ce que j'ai scrupuleusement vérifié avant d'y renoncer (§5)

Le §5 exigeait de sonder l'outillage avant tout. Résultat :

```
GPU_ARCH=NVIDIA_GB10_compute_cap_12.1
DRIVER=580.173.02
CUDA=13.2 (build cuda_13.2.r13.2/compiler.37953736_0)
PROFILER_VERSION=Nsight_Compute_2026.1.1.0 (build 37634170)
NSIGHT_SYSTEMS=2025.6.3
ncu=/usr/local/cuda/bin/ncu              PRESENT
nsys=/usr/local/bin/nsys                 PRESENT
nvprof,nv-nsight-cu-cli                  ABSENTS
RmProfilingAdminOnly=1
```

Le refus, reproduit **trois fois** sur des métriques de complexité croissante :

```
ncu --metrics gpu__time_duration.sum        -> ERR_NVGPUCTRPERM
ncu --metrics sm__cycles_elapsed.avg        -> ERR_NVGPUCTRPERM
ncu --set basic                             -> ERR_NVGPUCTRPERM
```

```
==ERROR== ERR_NVGPUCTRPERM - The user does not have permission to access
NVIDIA GPU Performance Counters on the target device 0.
```

Même `gpu__time_duration.sum` — la durée du noyau, l'information la plus élémentaire — est refusée.
Le refus est donc **au niveau du pilote**, pas de la métrique.

Tentatives d'élévation, toutes infructueuses :

```
uid=1000 (non root)
/proc/driver/nvidia/params : lecture seule, NON accessible en ecriture
sudo sans mot de passe     : INDISPONIBLE
```

```
AVAILABLE_METRIC_MANIFEST=NOT_OBTAINABLE_WITHOUT_COUNTER_ACCESS
AVAILABLE_METRIC_MANIFEST_SHA256=NON_PRODUIT
   -> je ne fabrique pas un manifeste de metriques que je n ai pas pu interroger. C est
      exactement ce que le paragraphe 5 interdit en demandant de ne pas inventer de noms de
      metriques non supportes.
```

## 3 — La seule voie restante ne répond pas à la question

`nsys` **fonctionne** : il utilise le traçage d'activité CUPTI, qui n'exige pas les compteurs de
performance. Mais ce qu'il rend est une **durée**, c'est-à-dire ce que je possède déjà par
événements CUDA :

```
 ** CUDA GPU Kernel Summary (cuda_gpu_kern_sum):
      Time (%)  Total Time (ns)  Instances  Avg (ns)  Med (ns)  Min (ns)  Max (ns)
          69,0        7 008 064        160  43 800,0  46 608,0     4 480    78 368   k_kbx_runtime
          30,0        3 127 904         40  78 197,0  77 936,0    77 472    84 096   k_full_native
```

Aucune des familles exigées au §6 n'est couverte : pas d'instructions émises, pas de warps
éligibles, pas de stalls de scoreboard, pas de trafic L1/L2/DRAM, pas d'activité de pipeline DP4A.

```
NSYS_AVAILABLE_ACTIVITY=KERNEL_DURATION_ONLY
NSYS_ADDS_NO_CAUSAL_INFORMATION_BEYOND_D12_TIMING=YES
NSYS_PRESENTS_ITSELF_AS_A_COUNTER_SUBSTITUTE=NO
```

## 4 — Les trois contrastes restent, mais non instrumentés

Les contrastes du §4 sont gelés et valides comme **contrastes de paroi**. Ils attendent leurs
compteurs :

| | contraste | paroi | ce qu'il faut discriminer |
|---|---|---|---|
| **C1** | `KBX00` → `G0_D1` | **+14,13 µs** | émission/dépendance vs coût mémoire fixe |
| **C2** | `G0_D1` → `G0_D8` | **+0,295 µs** | ce qui peut varier fortement sans que la paroi bouge |
| **C3** | `E2_SAME_WARP` → `E2_DIFFERENT_WARPS` | **−1,9319 µs** | recouvrement de latence vs travail supplémentaire |

C1 est le plus informatif : **quatre lanes** deviennent actives et coûtent 14,13 µs. C2 est le
discriminant : 8× plus de lanes pour +0,30 µs.

## 5 — Les familles de métriques qui seraient interrogées (noms réels, non testés)

Le §6 énumère les catégories. Je consigne les **noms Nsight Compute correspondants**, en précisant
qu'ils n'ont **pas** pu être vérifiés sur cette machine — contrairement à ce que le §5 exige avant
usage :

```
NOMS_NON_VERIFIES_SUR_CETTE_MACHINE=YES

warp instructions            : smsp__inst_executed.sum , smsp__inst_issued.sum
eligible warps               : smsp__warps_eligible.avg.per_cycle_active
active warps                 : sm__warps_active.avg.pct_of_peak_sustained_active
issue-slot utilization       : smsp__issue_active.avg.pct_of_peak_sustained_active
integer / ALU pipeline       : smsp__pipe_alu_cycles_active.avg.pct_of_peak_sustained_active
DP4A pipeline                : smsp__inst_executed_pipe_* (nom exact a confirmer)
global loads                 : l1tex__t_sectors_pipe_lsu_mem_global_op_ld.sum
L1                           : l1tex__data_bank_conflicts_pipe_lsu.sum , l1tex__throughput.avg.pct_...
L2                           : lts__t_sectors.sum , lts__throughput.avg.pct_of_peak_sustained_elapsed
DRAM                         : dram__bytes.sum , dram__throughput.avg.pct_of_peak_sustained_elapsed
long scoreboard              : smsp__average_warps_issue_stalled_long_scoreboard_per_issue_active.ratio
short scoreboard             : smsp__average_warps_issue_stalled_short_scoreboard_per_issue_active.ratio
wait / dependency            : smsp__average_warps_issue_stalled_wait_per_issue_active.ratio
barrier                      : smsp__average_warps_issue_stalled_barrier_per_issue_active.ratio
branch / divergence          : smsp__average_warps_issue_stalled_branch_resolving_per_issue_active.ratio
```

Ces noms proviennent de la nomenclature Nsight Compute et devront être **confrontés au manifeste
réel** (`ncu --query-metrics`) dès que l'accès sera rétabli. Le §5 est clair : ne pas inventer de
métriques non supportées. Je les donne donc comme cibles à vérifier, pas comme manifeste.

## 6 — Comment débloquer (action requise sur la machine, hors de ma portée)

Le message du pilote pointe `developer.nvidia.com/ERR_NVGPUCTRPERM`. La voie standard :

```
1. editer /etc/modprobe.d/nvidia.conf et ajouter :
      options nvidia NVreg_RestrictProfilingToAdminUsers=0
2. recharger le module nvidia (redemarrage) ou redemarrer la machine
3. relancer la commande ncu ; verifier avec : ncu --query-metrics | head
```

Cela nécessite **root** et un redémarrage : je ne peux pas le faire, et je ne le tente pas sans
votre accord explicite.

Alternative : exécuter la campagne sous un compte disposant des droits (`sudo -E ncu …`), ce qui
n'est pas possible ici (`sudo` demande un mot de passe).

## 7 — Ce que R13 change dans le reste du programme

Rien de scientifique : les conclusions de R12 et de R11 tiennent sur la paroi mesurée, et R13 devait
seulement **expliquer** le mécanisme, pas le mesurer autrement.

```
ENGINE007_PRIMARY_LEVER=WARP_EPISODE_LEVEL_STRUCTURED_EXECUTION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
   -> conditionne a REAL_MODEL_EPISODES_CAN_BE_DEACTIVATED ou
      NATIVE_EPISODES_CAN_BE_REDISTRIBUTED_WITHOUT_FIDELITY_LOSS
ENGINE007C_STATUS=BLOCKED
PRODUCT_GAIN=NOT_MEASURED

BRANCHE_O1=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION  (design axis, apres analyse de faisabilite)
BRANCHE_O2=REDISTRIBUTE_NATIVE_EPISODES_ACROSS_MORE_PHYSICAL_WARPS
   -> R12 mesure deja 2 episodes / 2 warps < 2 episodes / 1 warp.
      Experiment: 6 episodes / 6 warps contre 6 episodes / 4 warps.
      Necessite une geometrie qualifiee separement. NON IMPLEMENTE ICI.
```

**Point important** : la branche O2 ne dépend **pas** des compteurs. R12 la motive directement par
une mesure de paroi (−1,93 µs). Elle est donc faisable même si R13 reste bloqué — c'est la seule
piste du programme qui ne soit pas en attente d'un accès matériel.

## 8 — HARNESS_DEFECT_43 (§21)

```
HARNESS_DEFECT_43=POSITION_RANGE_DIAGNOSTIC_CONFOUNDED_BY_HETEROGENEOUS_ARM_COMPOSITION
RESOLUTION=USE_WITHIN_ARM_POSITION_EFFECT_OR_PAIRED_CONTRAST_POSITION_EFFECT
STATUS=RESOLVED_FOR_FUTURE_CAMPAIGNS
   -> la plage des moyennes par position ne doit plus etre utilisee comme mesure de derive des
      lors que la dispersion inter-bras est forte. Remplacer par l effet de position intra-bras,
      ou par l effet de position sur les contrastes apparies (qui est l unite d analyse reelle).
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 9 — Un défaut que je dois consigner sur mon propre rapport R12

Le §2 de ce brief a raison de demander la correction. En vérifiant, je constate que le
`MAX_RESIDUAL=0,26 µs` publié en R12 était **faux** : mon script ajustait le modèle sans inclure E6
dans le jeu de points servant au calcul du résidu maximal, ou comparait à une référence différente.
Le modèle annoncé prédit 76,4 µs pour E6, qui en mesure 81,9.

```
HARNESS_DEFECT_44=R12_LINEAR_MODEL_RESIDUAL_UNDERSTATED
   -> le residu maximal publie (0,26 us) ne correspond pas a l ecart reel du modele sur E6 (5,5 us).
      Cause : le calcul du residu portait sur un sous-ensemble de points. La conclusion qualitative
      de R12 (le nombre d episodes est la variable primaire) n est pas affectee, mais la
      comptabilite lineaire publiee etait fausse et est retiree.
   STATUS=RESOLVED_BY_R13_CORRECTION
```

C'est la deuxième fois dans cette série qu'une erreur de mon harnais d'analyse — et non de mesure —
produit un chiffre trop favorable. Je le signale parce que le motif compte.

## 10 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE
AUCUN NOUVEAU MASQUE, AUCUN NOUVEAU NOYAU, AUCUNE INSTRUMENTATION DANS LE CODE CHRONOMETRE
   -> conformement au paragraphe 14. Les contre-expertises ont utilise les binaires R12 deja
      qualifies : bin/e7a-r12 et la table e829d189c74ab2623b7eef9e15bc454ff887125347760934fcd54990e0e6bb94.
COUNTER_RUN_TIMINGS=NONE_COLLECTED
AUCUNE DONNEE DE COMPTEUR N A ETE SIMULEE, ESTIMEE OU RECONSTRUITE
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 11 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R12_STATUS=CLOSED_ACTIVE_WARP_EPISODE_COUNT_PRIMARY_WALL_VARIABLE
LANE_DENSITY_WITHIN_ACTIVE_WARP_EPISODE=NOT_DOMINANT
ACTIVE_WARP_EPISODE_COUNT=PRIMARY_WALL_VARIABLE_FOR_NONZERO_KBX_PATH
ACTIVE_PHYSICAL_WARP_ASSIGNMENT=SECONDARY_MATERIAL_VARIABLE
ENGINE007A_R13_STATUS=BLOCKED_MISSING_HARDWARE_COUNTER_PERMISSION
EPISODE_ACTIVATION_COUNTER_SIGNATURE=UNRESOLVED
PHYSICAL_WARP_REUSE_COUNTER_SIGNATURE=UNRESOLVED
GPU_ARCH=NVIDIA_GB10_compute_cap_12.1
DRIVER=580.173.02
CUDA=13.2
PROFILER_VERSION=Nsight_Compute_2026.1.1.0
RmProfilingAdminOnly=1
NCU_ERROR=ERR_NVGPUCTRPERM_ON_ALL_METRICS_INCLUDING_gpu__time_duration
NSYS_AVAILABLE=YES_BUT_KERNEL_DURATION_ONLY
AVAILABLE_METRIC_MANIFEST=NOT_OBTAINABLE
AVAILABLE_METRIC_MANIFEST_SHA256=NON_PRODUIT
COUNTER_RUN_TIMINGS=NONE_COLLECTED
R4_STYLE_LINEAR_BOOKKEEPING=RETIRE
UNIVERSAL_0.2_PERCENT_AGREEMENT_CLAIM=RETIRE
EPISODE_SCALING_APPROX=~12.2_TO_12.6_US_PER_ADDITIONAL_ACTIVE_EPISODE
HISTORICAL_PRE_REGISTRATION_12.6380_US_PER_EPISODE=SUPPORTED_AS_ORDER_OF_MAGNITUDE_REFERENCE
KERNEL_LEVEL_6_TO_4_EPISODES=31.8_PERCENT
KERNEL_LEVEL_6_TO_3_EPISODES=46.0_PERCENT
KERNEL_LEVEL_6_TO_2_EPISODES=60.0_TO_62.2_PERCENT
KERNEL_LEVEL_6_TO_1_EPISODE=74.8_PERCENT
KERNEL_LEVEL_CONTRASTS_ONLY=YES
PRODUCT_TOK_S_GAIN=NOT_MEASURED
ENGINE007_PRIMARY_LEVER=WARP_EPISODE_LEVEL_STRUCTURED_EXECUTION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED
BRANCHE_O1=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION_AFTER_FEASIBILITY
BRANCHE_O2=REDISTRIBUTE_EPISODES_ACROSS_MORE_PHYSICAL_WARPS
BRANCHE_O2_DEPENDS_ON_COUNTERS=NO
REMEDIATION_REQUIRED=NVreg_RestrictProfilingToAdminUsers_0_ET_REDEMARRAGE
REMEDIATION_NEEDS_ROOT=YES
HARNESS_DEFECT_43=RESOLVED_FOR_FUTURE_CAMPAIGNS
HARNESS_DEFECT_44=R12_LINEAR_MODEL_RESIDUAL_UNDERSTATED_RESOLVED_BY_R13_CORRECTION
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
