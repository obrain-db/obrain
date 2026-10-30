# ENGINE-007B-F1 — FAISABILITÉ DE L'ÉLIMINATION STRUCTURÉE ALIGNÉE SUR LES ÉPISODES

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1/`

---

## 0 — Résultat

**F1 est bloqué, mais pour une raison qui n'est pas un manque de moyens : la prémisse
géométrique du brief ne correspond pas au modèle réel.**

```
ENGINE007B_F1_STATUS=BLOCKED_PREMISE_GEOMETRY_MISMATCH_AND_MISSING_ACTIVATION_HARNESS
```

Trois faits établis, et un obstacle :

| | |
|---|---|
| **1.** Modèle cible réel identifié et présent | `Qwen3.8-27B-OBRAIN-v3.gguf`, sha `1d7c9409…`, 12,8 Go, 866 tenseurs |
| **2.** Géométrie épisode → poids **prouvée par la source** | 1024 poids contigus par épisode, aucune ambiguïté |
| **3.** Population TQ2_G128 **inventoriée** | **168 tenseurs, 2 113 536 lignes**, blocs 8..55 |
| **4.** **Obstacle** | les tenseurs dominants ont **K=5120 → 5 épisodes**, pas la géométrie canonique à 6 épisodes du brief |

Le masque 6 bits du §15 (« un bit par épisode ») **ne s'applique pas** aux tenseurs qui portent
l'essentiel du travail. Un ajustement de périmètre est nécessaire avant de continuer.

---

## 1 — Freeze des résultats antérieurs (§1, §2, §3)

```
ENGINE007O2_STATUS=CLOSED_SIX_WARP_GEOMETRY_REJECTED
NATIVE_MEAN_US=83.2744   O2_6WARP_MEAN_US=84.1012   O2_MINUS_NATIVE_US=+0.8268
KERNEL_WALL_REDUCTION_PERCENT=-0.9929   KERNEL_SPEEDUP=0.990169
DENSE_EPISODE_REDISTRIBUTION=REJECTED_AT_FULL_SIX_EPISODE_GEOMETRY
R12_E2_SAME_VS_DIFFERENT_EFFECT=DOES_NOT_GENERALIZE_MATERIALLY_TO_FULL_GEOMETRY
   -> aucun balayage 5/7/8 warps ; ENGINE007O2-PROD1 non ouvert.

R2-SANE : retirer tous les sites IDP.4A  -> 3,22 % de paroi
R10     : retirer ~75 % des corps kbx    -> 2,54 % de paroi
R11/R12 : KBX00=6,21 ; 1 episode=20,3 ; 2=31-33 ; 3=44,2 ; 4=55,9 ; 6=81,9 us
          dans un episode : 4 lanes=20,33 ; 32 lanes=20,62 us

ACTIVE_WARP_EPISODE_COUNT=PRIMARY_WALL_VARIABLE_FOR_NONZERO_KBX_PATH
LANE_DENSITY_WITHIN_ACTIVE_WARP_EPISODE=NOT_DOMINANT
UNSTRUCTURED_WORK_CARDINALITY=NOT_A_MATERIAL_LEVER

KERNEL_LEVEL_TOPOLOGY_CONTRASTS (pas des gains produit) :
   6 -> 4 : 31,8 %   6 -> 3 : 46,0 %   6 -> 2 : 60-62 %   6 -> 1 : 74,8 %
```

## 2 — Identité du modèle (§5, §11)

**Le fichier désigné par le brief n'est pas le modèle complet.**

```
BRIEF_INDIQUE : mtp-Qwen3.8-27B-TQ2G128.gguf
   -> 799 304 992 octets, 18 tenseurs, UN SEUL bloc (blk.64), 10 TQ2_G128
   -> c'est la TETE MTP (nextn), pas les 27B
```

Inventaire de tous les GGUF de la machine : **cinq têtes MTP** (5×18 tenseurs, blocs `64..64`) et
**un seul artefact portant le modèle entier en TQ2_G128** :

```
MODEL_PATH=/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
MODEL_BYTES=12844231424
GGUF_VERSION=3   N_TENSORS=866

METADATA general.architecture=qwen35
METADATA general.name=Qwen3.8-27B        METADATA general.size_label=27B
METADATA general.basename=Qwen3.8-27B    METADATA general.quantized_by=Unsloth
METADATA general.base_model.0.name=Qwen3.8 27B
METADATA qwen35.block_count=65           METADATA qwen35.embedding_length=5120
METADATA qwen35.feed_forward_length=17408   METADATA qwen35.nextn_predict_layers=1
METADATA qwen35.attention.head_count=24  METADATA qwen35.attention.head_count_kv=4
METADATA qwen35.ssm.inner_size=6144      METADATA qwen35.ssm.state_size=128
METADATA qwen35.context_length=262144

Q4N_MODEL_FAMILY=Qwen3.8    Q4N_MODEL_VARIANT=27B
Q4N_LLAMA_CPP_ARCH_ENUM=LLM_ARCH_QWEN35   Q4N_LLAMA_CPP_ARCH_ID=qwen35
LLAMA_CPP_ARCH_ID_IS_MODEL_VERSION=NO     ACTIVE_MODEL_IS_QWEN3_8_27B=YES
QWEN3_5_USED_FOR_EXECUTION=NO
```

L'architecture est confirmée comme **`qwen35` (Qwen3.8-27B)** : 65 blocs, dimension 5120, FFN 17408,
24 têtes / 4 KV, avec une composante SSM (inner_size 6144, state 128) — c'est un modèle hybride
attention/SSM, ce qui explique que certains rôles attendus n'existent pas (§4).

## 3 — Géométrie épisode → poids, **prouvée** par la source (§7, §8)

Le §7 interdit de supposer que les épisodes correspondent à des dimensions K contiguës. Voici la
preuve, tirée du code, et non d'une hypothèse :

```
vecdotq.cuh:114   #define VDR_TQ2_G128_Q8_1_MMVQ 1
mmvq.cu:42        get_vdr_mmvq(TQ2_G128) = VDR_TQ2_G128_Q8_1_MMVQ = 1
                  (le commentaire source precise : « une tranche de 32 par appel »)
                  -> qi = QK_G128 / QK8_1 = 128 / 32 = 4
mmvq.cu:518-519   blocks_per_row_x = ncols_x / qk          (qk = QK_G128 = 128)
                  blocks_per_iter  = vdr * nwarps * warp_size / qi
                                   = 1 * 4 * 32 / 4 = 32
mmvq.cu:606       for (int kbx = tid/(qi/vdr); kbx < blocks_per_row_x; kbx += blocks_per_iter)
mmvq.cu:732       vec_dot_tq2_g128_q8_1(vbq, bq8_1, kbx, kqs) :
                      const block_tq2_g128 * bq = (const block_tq2_g128 *) vbq + kbx;
                  -> le bloc kbx est indexe LINEAIREMENT dans la ligne de poids
```

**Conséquence, sans ambiguïté :**

```
EPISODE_LOGICAL_WEIGHT_GRANULARITY=1024_CONTIGUOUS_WEIGHTS
   kbx couvre EXACTEMENT [kbx*128 , kbx*128+128) du K logique
   episode G = kbx/8  ->  G couvre [G*1024 , G*1024+1024)
   aucun stride interne, aucun padding, aucun packing special
   48 kbx = 6144 poids couverts exactement, sans trou ni recouvrement
```

C'est le point favorable du dossier : **la granularité est propre et contiguë**. Éliminer un épisode
revient à supprimer exactement 1024 poids contigus du K logique d'une ligne.

## 4 — Obstacle principal : la géométrie canonique ne correspond pas au modèle (§8, §15)

Le §8 attend 48 kbx par ligne (6144 poids) et le §15 un masque de 6 bits (un par épisode).
**La géométrie dépend de K**, et K n'est pas uniforme dans le modèle :

| rôle | n | K | kbx/ligne | **épisodes actifs** | géométrie canonique 6 épisodes |
|---|---|---|---|---|---|
| `ffn_up.weight` | 48 | 5120 | 40 | **5** | NON |
| `ffn_gate.weight` | 48 | 5120 | 40 | **5** | NON |
| `attn_gate.weight` | 36 | 5120 | 40 | **5** | NON |
| `attn_q.weight` | 12 | 5120 | 40 | **5** | NON |
| `attn_k.weight` | 12 | 5120 | 40 | **5** | NON |
| `attn_output.weight` | **12** | **6144** | **48** | **6** | **OUI** |

**156 tenseurs sur 168 — et 2 007 040 lignes sur 2 113 536 (95,0 %) — sont à K=5120, donc à
5 épisodes.** Seul `attn_output` (12 tenseurs, 106 496 lignes, 5,0 %) a la géométrie à 6 épisodes
du brief.

Géométrie réelle pour K=5120, dérivée du même mapping natif :

```
kbx = tid/4 + 32*i ; 40 kbx -> i in {0,1}
  i=0 : kbx 0..31  -> 128 threads actifs
  i=1 : kbx 32..39 -> tid/4 < 8 -> tid < 32 -> 32 threads actifs (warp 0 SEUL)

  G0 : kbx  0.. 7   warp 0   32 threads
  G1 : kbx  8..15   warp 1   32 threads
  G2 : kbx 16..23   warp 2   32 threads
  G3 : kbx 24..31   warp 3   32 threads
  G4 : kbx 32..39   warp 0   32 threads
  G5 : kbx 40..47   INACTIF (kbx max = 39)

  ACTIVE_EPISODES=5        EPISODES_PER_WARP={2,1,1,1}
```

```
K5120_GEOMETRY={2,1,1,1}_SUR_4_WARPS_5_EPISODES
K6144_GEOMETRY={2,2,1,1}_SUR_4_WARPS_6_EPISODES   (= geometrie R11/R12)
BRIEF_6_BIT_MASK_APPLIES_TO=5_PERCENT_OF_ELIGIBLE_ROWS_ONLY
```

**Ce n'est pas une objection de forme.** Le masque de 6 bits et les niveaux KEEP6/KEEP5/KEEP4/KEEP3
du §18 supposent 6 épisodes. Appliqués tels quels aux tenseurs dominants, ils n'ont pas de sens :
il n'y a que 5 épisodes, et `KEEP5` y serait le **baseline**, pas une ablation.

## 5 — Population éligible (§9, §10, §11)

```
ELIGIBLE_TENSOR_COUNT=168
ELIGIBLE_ROW_COUNT=2113536
BLOCS_CONCERNES=48   plage=8..55
BLOCS_SANS_TQ2_G128=0..7 et 56..64  (17 blocs : les premiers, les derniers, et la tete MTP)
```

Répartition globale des types dans le modèle (866 tenseurs) :

```
F32=360   Q4_K=176   TQ2_G128=168   BF16=96   Q5_K=65   Q6_K=1
```

Les tenseurs stockés en Q4_K / Q5_K / Q6_K / F32 / BF16 **ne passent pas par le noyau TQ2_G128** :
ils ne sont pas concernés par l'élimination d'épisodes telle que mesurée.

*(Le code de type GGUF est `43` ; la table de noms standard s'arrête à 37 — `GGML_TYPE_TQ2_G128 = 43`
est défini dans le fork, `ggml.h:439`. Je le consigne parce que mon premier comptage a échoué sur ce
point, et que quiconque reprendra ce travail rencontrera le même piège.)*

## 6 — Écart entre les rôles attendus et les rôles réels (§9, §10)

Le §9 liste « `ffn_gate`, `ffn_up`, `attn_gate`, `attn_q`, `attn_k`, `attn_output` » — et demande
explicitement de **re-dériver depuis l'artefact courant plutôt que d'hériter**. Résultat :

| rôle attendu par le brief | présent en TQ2_G128 ? |
|---|---|
| `ffn_gate` | **oui** (48) |
| `ffn_up` | **oui** (48) |
| `attn_gate` | **oui** (36) |
| `attn_q` | **oui** (12) |
| `attn_k` | **oui** (12) |
| `attn_output` | **oui** (12) |
| **`ffn_down`** | **NON** — stocké en Q4_K |
| **`attn_v`** | **NON** — le modèle a un tenseur fusionné `attn_qkv` en Q5_K |
| **`attn_qkv`** | **NON** — Q5_K |

Les six rôles annoncés sont donc exacts, mais **`ffn_down` et `attn_v` cités dans mon propre rapport
d'attente ne le sont pas** : c'est la vérification demandée au §9 qui le montre, et je le corrige.

## 7 — La géométrie native confirme la fidélité de toutes les mesures 007A

Un point de méthode qui vaut d'être noté : la géométrie que j'ai **supposée** dans les microbenchs
R9–R12 (`kbx = tid/4 + 32*i`, 4 threads par kbx, pas de stride) est **exactement** celle du code
produit (`mmvq.cu:606`, `qi=4`, `vdr=1`). Les mesures 007A ne reposaient donc pas sur un modèle
faux de la boucle native — c'est ce qui rend crédible leur transposition au noyau réel.

## 8 — Ce qui bloque, et ce qu'il faut pour continuer

Les étapes §13 à §31 exigent la contribution par épisode `y_e = W_episode_e · x_episode_e` sur des
**activations réelles**. Or :

```
ACTIVATION_CAPTURE_TOOL=ABSENT
   -> outils disponibles : perplexity, llama-bench, llama-perplexity
      aucun ne capture les entrees de tenseur par couche
   -> le depot ne fournit aucun point d accroche de dump d activations
```

Il faut donc, avant de pouvoir continuer :

```
1. ARBITRER LA GEOMETRIE
   -> confirmer que F1 doit etre reformule a 5 episodes pour les tenseurs K=5120
      (masque de 5 bits, niveaux KEEP5=baseline / KEEP4 / KEEP3 / KEEP2)
      et garder 6 episodes pour attn_output seulement.                              [decision]

2. CONSTRUIRE LA CAPTURE D ACTIVATIONS
   -> un chemin non-produit qui enregistre, pour une matrice cible et un corpus fixe,
      le vecteur d entree x de chaque token. C est un travail de plomberie de harnais,
      pas une modification du noyau produit ni du modele.                           [travail]

3. ALORS SEULEMENT : contributions par episode, masques statiques, controles
   aleatoires, oracle dynamique, stabilite, ablation locale puis bout-en-bout.       [analyse]
```

Je **n'ai pas** construit cette capture dans cette passe, et je n'ai donc produit **aucune** des
mesures §14, §16, §17, §20, §21, §25, §26, §27, §29, §30. Je ne les estime pas, je ne les simule
pas, et je ne les remplace pas par une analyse de normes de poids — le §13 l'interdit
explicitement (« ne pas inferer l'importance d'un episode depuis la magnitude des poids seule »).

## 9 — Statut selon le §41

Le §41 impose un choix parmi quatre états :

```
FEASIBLE_CANDIDATE | FEASIBLE_BUT_LIMITED | STATIC_SELECTION_INSUFFICIENT
| NOT_FEASIBLE_AT_NATIVE_EPISODE_GRANULARITY
```

**Aucun des quatre n'est applicable** : ils sont tous des verdicts sur la qualité de la sélection
d'épisodes, et cette question n'a pas encore été posée. Je ne choisis donc pas un état que la mesure
ne soutient pas :

```
ENGINE007B_F1_STATUS=BLOCKED_PREMISE_GEOMETRY_MISMATCH_AND_MISSING_ACTIVATION_HARNESS
EPISODE_ALIGNED_STRUCTURED_ELIMINATION=NOT_DETERMINED
PRODUCT_GAIN=NOT_MEASURED
```

## 10 — Livrables produits

`qualification/e7b-f1/` :

```
EPISODE_GEOMETRY.txt          geometrie prouvee par la source + population TQ2_G128
TENSOR_ELIGIBILITY.csv        169 lignes : 168 tenseurs eligibles + en-tete
gguf_dump_cible.json          vidage GGUF complet du modele cible (metadonnees + 866 tenseurs)
gguf_enum.py                  lecteur GGUF autonome ecrit pour F1 (aucune dependance externe)
CONFIG_PROVENANCE.txt         provenance modele, outillage, decisions
```

**Non produits** (dépendent de la capture d'activations) : `EPISODE_CONTRIBUTIONS.*`,
`STATIC_MASKS.csv`, `MASK_STABILITY.csv`, `LOCAL_ABLATION_METRICS.csv`,
`LAYER_ROLE_SENSITIVITY.csv`, `END_TO_END_ABLATION.csv`, `ACTIVE_EPISODE_DISTRIBUTION.csv`,
`MICROKERNEL_WALL_PROJECTION.txt`.

## 11 — Aucune mutation (§6, §38)

```
MODELE_MODIFIE=NON      GGUF_ECRASE=NON      NOUVEL_ARTEFACT_QUANTIFIE=NON
NOYAU_PRODUIT_MODIFIE=NON                    (mmvq.cu sha 2bf9a270… inchange)
GEOMETRIE_DU_MODELE_MESUREE=NON
   -> aucun masque n a ete propose, donc aucun a stocker (paragraphe 38).
```

## 12 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R12_STATUS=CLOSED_ACTIVE_WARP_EPISODE_COUNT_PRIMARY_WALL_VARIABLE
ENGINE007A_R13_STATUS=BLOCKED_MISSING_HARDWARE_COUNTER_PERMISSION
ENGINE007O2_STATUS=CLOSED_SIX_WARP_GEOMETRY_REJECTED
DENSE_EPISODE_REDISTRIBUTION=REJECTED_AT_FULL_GEOMETRY
ACTIVE_WARP_EPISODE_COUNT=PRIMARY_WALL_VARIABLE_FOR_NONZERO_KBX_PATH
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED
PRODUCT_GAIN=NOT_MEASURED

ENGINE007B_F1_STATUS=BLOCKED_PREMISE_GEOMETRY_MISMATCH_AND_MISSING_ACTIVATION_HARNESS
MODEL_FOR_F1=Qwen3.8-27B-OBRAIN-v3.gguf
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
BRIEF_DESIGNATED_FILE_IS_AN_ARTIFACT=mtp-Qwen3.8-27B-TQ2G128.gguf_IS_THE_MTP_HEAD_ONLY
EPISODE_LOGICAL_WEIGHT_GRANULARITY=1024_CONTIGUOUS_WEIGHTS
GEOMETRY_PROVEN_BY_SOURCE=YES_mmvq.cu_606_plus_vecdotq.cuh_732
ELIGIBLE_TENSOR_COUNT=168
ELIGIBLE_ROW_COUNT=2113536
ELIGIBLE_BLOCKS=48_range_8_to_55
TQ2_G128_GGUF_TYPE_CODE=43
K5120_TENSOR_COUNT=156
K5120_EPISODES=5
K5120_EPISODE_MAPPING={2,1,1,1}_ON_4_WARPS
K6144_TENSOR_COUNT=12
K6144_EPISODES=6
K6144_EPISODE_MAPPING={2,2,1,1}_ON_4_WARPS
BRIEF_6_BIT_MASK_COVERAGE=5.0_PERCENT_OF_ELIGIBLE_ROWS
BRIEF_KEEP_LEVELS_INAPPLICABLE_TO_95_PERCENT_OF_ROWS=YES
ACTIVATION_CAPTURE_TOOL=ABSENT
CALIBRATION_CORPUS=NOT_ESTABLISHED
PER_EPISODE_CONTRIBUTION_MEASURED=NO
STATIC_MASK_SELECTION=NOT_PERFORMED
LOCAL_ABLATION=NOT_PERFORMED
END_TO_END_ABLATION=NOT_PERFORMED
EPISODE_ALIGNED_STRUCTURED_ELIMINATION=NOT_DETERMINED
ACTIVATION_AWARE_CALIBRATION_REQUIRED=NOT_DETERMINED
MODEL_MUTATED=NO
PRODUCTION_KERNEL_MODIFIED=NO
NEXT_STEP=SCOPE_DECISION_ON_5_EPISODE_GEOMETRY_THEN_BUILD_ACTIVATION_CAPTURE
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
