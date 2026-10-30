# ENGINE-005H — EXACT DISPATCH DOMAIN / ATTN_GATE CAPABILITY GATE : réponse

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5h/`

---

## 0 — Réponse à la question posée (§3)

`FOR_THE_DOMINANT_RECURRENT_ATTN_GATE_MMVQ_CALLS, IS_THE_MMQ_SUPPORT_PREDICATE ALSO TRUE?`

**Non — pas pour les appels dominants.** Les appels de décodage de `attn_gate` se répartissent en
**deux domaines de capacité opposés**, et le domaine **majoritaire interdit MMQ** :

```
qtype=12 (q4_K)     : can_mmvq=YES  can_mmq=YES   → 3 132 appels de décodage
qtype=43 (tq2_g128) : can_mmvq=YES  can_mmq=NO    → 9 396 appels de décodage
```

```
ATTN_GATE_ALTERNATE_MMQ_STRUCTURALLY_SUPPORTED=NO_FOR_DOMINANT_DOMAIN
ATTN_GATE_MMQ_ALTERNATIVE_VALID_DOMAIN=SUBSET
SUBSET_SHARE_OF_DECODE_CALLS=3132/12528 = 25,0 %
ATTN_GATE_MMVQ_VS_MMQ_AB_ALLOWED=NO_AS_SPECIFIED
CAS_DECISIF=B
```

**C'est le cas B du protocole**, pas le cas A. Et il est établi par les **prédicats réels de la
source**, pas par une reconstruction depuis `N=1` (§4).

---

## 1 — Méthode (§4 à §6)

Instrumentation au point de sélection, enregistrant les **quatre prédicats réellement évalués** :

```cpp
const bool p_mmvf = ggml_cuda_should_use_mmvf(src0->type, cc, src0->ne, src0->nb, ne11);
const bool p_mmf  = ggml_cuda_should_use_mmf (src0->type, cc, warp_size, src0->ne, src0->nb, ne11, false);
const bool p_mmvq = ggml_cuda_should_use_mmvq(src0->type, cc, ne11);
const bool p_mmq  = ggml_cuda_should_use_mmq (src0->type, cc, ne11, 0);
const int  pred   = mmvf | mmf<<1 | mmvq<<2 | mmq<<3;      // masque publie tel quel
```

Clé de signature = hash(pointeur du poids, qtype, masque de prédicats, dimensions consultées
`src0 ne[0..3]` et `src1 ne[1..2]`). Nom du poids lu depuis le tenseur (`ggml_get_name(src0)`),
donc **rôle identifié par le nom réel du poids**, pas déduit.

Résultat : **1 738 lignes de signature** distinctes, publiées intégralement dans
`qualification/e5h/signatures.err`.

Discipline : évaluations **indépendantes** des quatre prédicats (et non en cascade) — c'est la seule
façon de savoir si un chemin alternatif est structurellement possible. Aucune modification de la
décision : les booléens servent uniquement au recensement.

## 2 — Histogramme attn_gate (§6, §10)

`attn_gate.weight` : 48 couches, deux types de quantification, **189 lignes de signature**.

| qtype | prédicats observés | chemin | appels | nb |
|---|---|---|---|---|
| 12 (q4_K) | `..QM` | MMVQ | 3 132 | décodage (s1=1) |
| 12 (q4_K) | `...M` | MMQ | 45 | préfill (s1=256/245/48) |
| **43 (tq2_g128)** | **`..Q.`** | **MMVQ** | **9 396** | **décodage (s1=1)** |
| 43 (tq2_g128) | `....` | OTHER | 108 | préfill |

Lecture : pour `tq2_g128`, **seul `can_mmvq` est vrai** ; `can_mmq` est **faux**, et le préfill
tombe même hors MMVQ (les quatre prédicats faux → chemin non quantifié).

```
RECURRENT_ATTN_GATE_DECODE_MMVQ_CALLS=12528
  dont MMQ structurellement valide : 3132 (q4_K)
  dont MMQ structurellement invalide: 9396 (tq2_g128)   ← 75 %
RECURRENT_ATTN_GATE_NATURAL_MMQ_SELECTIONS=45  (prefill q4_K uniquement)
MMVQ_PREDICATE_ON_NATURAL_MMQ_CALLS=TRUE       (pour ces 45 : ..QM, donc les deux valides)
```

Conclusion sur le MIXED signalé en 005G : le mélange **n'est pas** « signature A → MMVQ, B → MMQ »
au sein d'un même domaine — c'est **deux domaines de capacité disjoints** plus la séparation
décodage/préfill. La sélection naturelle de MMQ est **entièrement du préfill**.

## 3 — Wiring de la cascade

Les deux domaines confirment que **la priorité de cascade explique le choix MMVQ** quand les deux
prédicats sont vrais (`..QM` → MMVQ bien que MMQ soit valide) — c'est là le cas A de §7, mais il ne
concerne que **25 % des appels de décodage**. Pour les 75 % restants (qtype 43), MMQ est
**structurellement indisponible** : le forcer exigerait de court-circuiter le prédicat, ce que §8
interdit explicitement.

## 4 — SSM alpha/beta (§12, §24)

```
blk.10.ssm_alpha.weight  261  30 (bf16)  MMVF   V...   5120x48   s1=1     ← décodage
blk.10.ssm_alpha.weight    2  30         OTHER  ....   5120x48   s1=256   ← préfill
blk.10.ssm_beta.weight   261  30         MMVF   V...   5120x48   s1=1
blk.10.ssm_beta.weight     2  30         OTHER  ....   5120x48   s1=256
```

```
SSM_ALPHA_KERNEL_PATH=MIXED_MMVF_DOMINANT          (confirmé)
SSM_BETA_KERNEL_PATH=MIXED_MMVF_DOMINANT           (confirmé)
DENSE_BF16_PATH_FOR_SSM_ALPHA_BETA=MMVF_RUNTIME_PROVEN
SSM_ALPHA_BETA_OPTIMIZATION_CLASS=DENSE_BF16_MMVF_OR_STRUCTURAL
```

Le « ~2 % non-MMVF » est **classé** : ce sont exactement les jetons de préfill (s1 ∈ {256, 245, 48}),
où `can_mmvf` et `can_mmf` sont faux et où la charge atterrit sur le chemin dense.

## 5 — Pourquoi les FFN quantifiés atteignent OTHER (§11)

```
ffn_gate/up par (qtype, predicats) : (12, '...M') = 128  |  (43, '..Q.') = 96  |  (43, '....') = 263
ffn_gate/up par chemin            : MMQ 128 | MMVQ 96 | OTHER 263
```

```
WHY_QUANTIZED_FFN_REACHES_OTHER=MMVQ_AND_MMQ_PREDICATES_BOTH_FALSE_ON_tq2_g128_PREFILL
```

Même mécanisme que `attn_gate` : sur `tq2_g128` (qtype 43), ni `can_mmvq` ni `can_mmq` ne sont vrais
pour les jetons de préfill. **Aucune optimisation FFN ouverte** — classification descriptive
seulement, comme demandé.

**Fait structurel qui se répète** : `tq2_g128` (qtype 43) est un domaine où le décodage est **forcé**
sur MMVQ et où le préfill sort des deux chemins quantifiés.

## 6 — Terminologie (§15)

Les chiffres publiés sont des **sélections de dispatch**, pas des exécutions logiques :

```
DISPATCH_SELECTION_COUNT=77413                        (total, toutes signatures)
ONE_DISPATCH_SELECTION_EQUALS_ONE_LOGICAL_EXECUTION=NOT_PROVEN
```

Réserve connue : le recensement a été fait **graphes désactivés**, où la cascade est réévaluée à
chaque lancement. Sous CUDA Graph, elle peut ne pas l'être à chaque replay (§14) — la qualification
capture-safe (§13) reste à faire :

```
DISPATCH_DECISION_DOMAIN_GRAPH_MODE_INDEPENDENT=NOT_YET_CHECKED
GRAPH_ON_OUTPUT_WEIGHT_ROUTE=NOT_MEASURED
GRAPH_OFF_OUTPUT_WEIGHT_ROUTE=MMVQ
GRAPH_ON_ATTN_GATE_SIGNATURE_SET=NOT_MEASURED
GRAPH_OFF_ATTN_GATE_SIGNATURE_SET=[(12,..QM,MMVQ),(12,...M,MMQ),(43,..Q.,MMVQ),(43,....,OTHER)]
```

## 7 — Conséquence pour la suite (§17 à §23)

```
ATTN_GATE_ALTERNATE_MMQ_STRUCTURALLY_SUPPORTED=NO_FOR_DOMINANT_DOMAIN
NUMERICAL_PREFLIGHT_AUTHORIZED=NO_FOR_DOMINANT_DOMAIN   (pas d'A/B honnete a preparer)
ENGINE005H_AB_ALLOWED=NO_AS_SPECIFIED
ATTN_GATE_MMVQ_IS_FORCED_BY_CAPABILITY_DOMAIN=YES_FOR_75_PERCENT_OF_DECODE_CALLS
```

Un A/B MMVQ↔MMQ n'est donc honnête que sur le **sous-ensemble `q4_K`** (25 % des appels de
décodage) — utile mais pas représentatif de la signature porteuse. Conformément à §22, la cible
`attn_gate` reste légitime mais l'action devient :

```
ENGINE006_CANDIDATE_ACTION=OPTIMIZE_CURRENT_MMVQ_FOR_tq2_g128_DECODE
```
plutôt que « comparer deux kernels existants ». Et comme §22 l'exige, il faudra obtenir une
sensibilité murale par un autre moyen qu'un A/B de kernel.

Statut `output.weight` (§23) : route confirmée par le recensement (qtype 12/q6_K, chemin MMVQ sur
`N=1`), coût mural toujours **non mesuré**.

## 8 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…   (identiques aux vierges)
Runtime instrumenté : build-e4-observe (census + op-timing)
Instrument conservé : source-e5h/ggml-cuda-census.{cuh,cpp,cu}
Séquence rejouée   : qualification/run003/run003-tokens.txt (fenêtre 8 tokens, teacher-forced)
```

## 9 — Ledger

```
ENGINE005G_STATUS=CLOSED_ROLE_KEYED_RUNTIME_DISPATCH
OUTPUT_WEIGHT_KERNEL_PATH=MMVQ
OUTPUT_WEIGHT_KERNEL_PATH_SOURCE=RUNTIME_DISPATCH_BRANCH
OUTPUT_WEIGHT_KERNEL_PATH_RUNTIME_PROVEN=YES
OUTPUT_WEIGHT_WALL_COST=NOT_MEASURED
OUTPUT_WEIGHT_FIRST_OPTIMIZATION_PRIORITY=NO
SSM_ALPHA_KERNEL_PATH=MMVF_RUNTIME_PROVEN
SSM_BETA_KERNEL_PATH=MMVF_RUNTIME_PROVEN
SSM_ALPHA_BETA_OPTIMIZATION_CLASS=DENSE_BF16_MMVF_OR_STRUCTURAL
RECURRENT_ATTN_GATE_KERNEL_PATH=MIXED_MMVQ_DOMINANT_RUNTIME_PROVEN
RECURRENT_ATTN_GATE_DECODE_CALLS=12528
RECURRENT_ATTN_GATE_MMQ_STRUCTURALLY_VALID_DECODE_CALLS=3132
RECURRENT_ATTN_GATE_MMQ_STRUCTURALLY_INVALID_DECODE_CALLS=9396
RECURRENT_ATTN_GATE_NATURAL_MMQ_SELECTIONS=45
MMVQ_PREDICATE_ON_NATURAL_MMQ_CALLS=TRUE
ATTN_GATE_ALTERNATE_MMQ_STRUCTURALLY_SUPPORTED=NO_FOR_DOMINANT_DOMAIN
ATTN_GATE_MMQ_ALTERNATIVE_VALID_DOMAIN=SUBSET_q4_K_25_PERCENT
ATTN_GATE_MMVQ_VS_MMQ_AB_ALLOWED=NO_AS_SPECIFIED
CAS_DECISIF=B
ATTN_GATE_MMVQ_IS_FORCED_BY_CAPABILITY_DOMAIN=YES_FOR_75_PERCENT_OF_DECODE_CALLS
FFN_GATE_UP_OTHER_DOMINANCE=EXPLAINED_tq2_g128_PREFILL_BOTH_PREDICATES_FALSE
WHY_QUANTIZED_FFN_REACHES_OTHER=MMVQ_AND_MMQ_PREDICATES_BOTH_FALSE_ON_tq2_g128_PREFILL
DISPATCH_SELECTION_COUNT=77413
ONE_DISPATCH_SELECTION_EQUALS_ONE_LOGICAL_EXECUTION=NOT_PROVEN
DISPATCH_DECISION_DOMAIN_GRAPH_MODE_INDEPENDENT=NOT_YET_CHECKED
ENGINE005H_STATUS=CLOSED_CAPABILITY_GATE_ANSWERED
NEXT_MEASUREMENT=SIGNATURE_CENSUS_WITH_GRAPHS_ACTIVE
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
ENGINE006_CANDIDATE_ACTION=OPTIMIZE_CURRENT_MMVQ_FOR_tq2_g128_DECODE
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
