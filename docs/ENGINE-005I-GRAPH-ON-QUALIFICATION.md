# ENGINE-005I — QUALIFICATION GRAPH-ON (volet 1) : réponse, et volet 2 spécifié

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5i/`

---

## 0 — État des deux volets

| volet | contenu | état |
|---|---|---|
| **1 — qualification graph-on** | mapping `(signature, domaine) → chemin noyau` avec CUDA Graph ACTIF | **FAIT, répond OUI** |
| **2 — sensibilité causale MMVQ natif** | duplication contrôlée du MMVQ ciblé (R0/R1/R2) | **SPÉCIFIÉ, non exécuté** (§4) |

```
ENGINE005I_STATUS=GRAPH_ON_QUALIFICATION_DONE_SENSITIVITY_PENDING
DISPATCH_DECISION_DOMAIN_GRAPH_MODE_INDEPENDENT=YES
ATTN_GATE_DISPATCH_GRAPH_MODE_INDEPENDENT=YES
TARGET_MMVQ_CAUSAL_WALL_SENSITIVITY=NOT_MEASURED
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
```

---

## 1 — Freeze 005H (§1 à §4)

```
ENGINE005H_STATUS=CLOSED_DISPATCH_DOMAIN
ATTN_GATE_DOMINANT_DECODE_DOMAIN=TQ2_G128_MMVQ_ONLY
  qtype=tq2_g128  decode s1=1  can_mmvq=YES  can_mmq=NO  selected=MMVQ
ATTN_GATE_ALTERNATE_MMQ_STRUCTURALLY_SUPPORTED=NO_FOR_DOMINANT_DOMAIN
ATTN_GATE_MMQ_ALTERNATIVE_VALID_DOMAIN=SUBSET_Q4_K_ONLY
ATTN_GATE_MMVQ_VS_MMQ_AB_ALLOWED=NO_AS_SPECIFIED
CAS_DECISIF=B
ATTN_GATE_MIXED_ROUTE_ROOT_CAUSE=DISJOINT_QTYPE_AND_SEQUENCE_DOMAINS
  (et non SAME_SIGNATURE_KERNEL_COMPETITION)
  tq2_g128 : DECODE → MMVQ, PREFILL → OTHER
  q4_K     : DECODE → MMVQ par PRIORITE (can_mmq=YES)
WHY_QUANTIZED_FFN_REACHES_OTHER=TQ2_G128_PREFILL_CAN_MMVQ_FALSE_AND_CAN_MMQ_FALSE
QUANTIZED_WEIGHT != GUARANTEED_QUANTIZED_SPECIALIZED_KERNEL
  la route depend de qtype × shape × geometrie de sequence × predicats backend
SSM_ALPHA_BETA_DOMINANT_KERNEL_PATH=MMVF_RUNTIME_PROVEN
SSM_ALPHA_BETA_DECODE_DOMAIN=MMVF     SSM_ALPHA_BETA_PREFILL_DOMAIN=OTHER
SSM_ALPHA_BETA_OPTIMIZATION_CLASS=DENSE_BF16_MMVF_OR_STRUCTURAL
ENGINE006_BRANCH_FFN=NOT_OPENED
```

## 2 — Volet 1 : protocole capture-safe (§5, §6)

Runtime dédié `build-e5i` construit depuis la source **vierge** avec **un seul** patch : le
recensement de dispatch. Aucun autre instrument :

```
SYMBOLES_TIMING_DANS_LE_RUNTIME=0     (verifie : obrain_op_timing=0, ggml_obrain_obs=0)
CUDA_EVENTS_INSTRUMENTES=0
TIMING_PAR_OPERATION=0
LOG_DANS_LE_CHEMIN_CHAUD=0            (2 atomiques relaxed par selection)
```

Fenêtre : 8 tokens teacher-forced, **CUDA Graph ACTIF** (warmup normal, `PROPERTIES_CHANGED_TRUE=0`
après pré-roll), `E5E_WALL_MS_PER_TOKEN=63,88` — régime produit.

Le recensement enregistre, à chaque évaluation de la cascade : rôle du poids (nom du tenseur),
qtype, forme `src0 ne[0..3]` et `src1 ne[1..2]`, tuple de prédicats, chemin retenu.

## 3 — Résultat : le mapping est indépendant du mode graphe

Comparaison **des mappings**, pas des comptes (§6) — clé = `(rôle, qtype, ne00, ne01, s1)`,
valeur = ensemble des couples `(prédicats, chemin)` :

```
SIGNATURES_GRAPH_OFF=1738
SIGNATURES_GRAPH_ON =1738
INTERSECTION=1738        SEULEMENT_OFF=0        SEULEMENT_ON=0
MAPPING_DIVERGENT=0
```

**Aucune divergence, aucune signature manquante, aucune signature nouvelle.** Vérifications ciblées,
toutes `IDENTIQUE_EN_ON=True` :

| signature | GRAPH_OFF | GRAPH_ON |
|---|---|---|
| `attn_gate` / qtype 43 / decode s1=1 (36 poids) | `..Q.` → **MMVQ** | `..Q.` → **MMVQ** |
| `output.weight` / qtype 14 (q6_K) / s1=1 | `..QM` → **MMVQ** | `..QM` → **MMVQ** |
| `ssm_alpha.weight` / qtype 30 / s1=1 (48 poids) | `V...` → **MMVF** | `V...` → **MMVF** |
| `ssm_beta.weight` / qtype 30 / s1=1 (48 poids) | `V...` → **MMVF** | `V...` → **MMVF** |
| `attn_q.weight` decode | qtype 12 → `..QM`/MMVQ (×4), qtype 43 → `..Q.`/MMVQ (×12) | identique |

```
ATTN_GATE_DISPATCH_GRAPH_MODE_INDEPENDENT=YES
DISPATCH_DECISION_DOMAIN_GRAPH_MODE_INDEPENDENT=YES   (pour les signatures observees)
```

Précision importante sur la portée : le recensement a bien été évalué en mode graphe — la cascade est
exécutée à chaque lancement de `ggml_backend_cuda_graph_compute`, y compris pendant la capture. Ce qui
est comparé ici est donc **le mapping**, conformément à §6 ; les **nombres** de sélections ne sont pas
comparés (le mode graphe ne réévalue pas la cascade à chaque replay).

La cible candidate est donc confirmée en régime produit (§7) :

```
ENGINE006_CANDIDATE_SIGNATURE=recurrent attn_gate.weight / tq2_g128 / decode s1=1 / MMVQ
ENGINE006_TARGET=NOT_PINNED                (jusqu'a la sensibilite murale)
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
```

## 4 — Volet 2 : sensibilité causale du MMVQ natif — spécification

Faute de kernel alternatif valide dans le domaine dominant (§8), la sensibilité doit venir d'une
**duplication contrôlée du kernel natif**. La méthode est spécifiée, pas exécutée.

### 4.1 Gate sémantique (à franchir AVANT toute mesure, §10)

Il faut **prouver par la source** que le MMVQ ciblé (`mul_mat_vec_q` pour `tq2_g128`) :

```
WRITES_OUTPUT=YES            (et non ACCUMULATE_INTO_EXISTING_OUTPUT)
F(F(x)) == F(x)              (plusieurs invocations ecrivent la meme sortie)
```

Si le kernel lit `dst`, alors `DUPLICATE_WORK_EXPERIMENT_ALLOWED=NO`.

### 4.2 Instrument

Trois variantes d'un même runtime, une seule constante de différence :

```
E6_MMVQ_EXTRA_REPEATS=0 | 1 | 2     lue au point de selection
cible : src0 == <pointeur attn_gate tq2_g128> ET chemin retenu == MMVQ
meme entree, meme poids, meme destination, meme flux, memes parametres
aucun scratch buffer (§12)
```

Compteurs exigés (§17) :

```
TARGET_NATIVE_MMVQ_COUNT      TARGET_EXTRA_MMVQ_COUNT      NON_TARGET_EXTRA_MMVQ_COUNT
R1 : TARGET_EXTRA = 1 × TARGET_NATIVE      R2 : 2 × TARGET_NATIVE      NON_TARGET_EXTRA=0
```

Si le compteur ne se déclenche qu'à la construction du graphe, publier en plus une **preuve
structurelle** du nombre de kernels supplémentaires réellement capturés (§17).

### 4.3 Endpoint et analyse

Fenêtre 256 tokens, `CUDA_GRAPH=ON`, même protocole que RUN-003
(`BACKEND_ACTIVE_INTERVAL_SUM_MS`, `BACKEND_EQUIVALENT_TPS`). Ordre alterné fixé avant observation
(`R0 R1 R2 R2 R1 R0`), mean/median/sd/range par niveau, puis :

```
DELTA_01 = R1 - R0      DELTA_12 = R2 - R1      DUPLICATE_WORK_LINEARITY_RATIO = DELTA_12 / DELTA_01
```

Interprétation (§21) : `R0 < R1 < R2` reproductible ⇒
`ATTN_GATE_TQ2_MMVQ_CAUSAL_WALL_SENSITIVITY=YES` ; dans le bruit ⇒ `NOT_DETECTED` ; très non
linéaire ⇒ `CACHE_OR_SCHEDULING_INTERACTION=LIKELY` et **pas** de conversion en coût natif.

Nommage : la pente s'appellera `NATIVE_MMVQ_DUPLICATE_WORK_WALL_SENSITIVITY`, **pas**
`NATIVE_ATTN_GATE_WALL_COST` (§13) — la duplication immédiate bénéficie de la résidence en cache.

### 4.4 Contrôle q4_K (§22, §23)

`Q4K_AB_CONTROL_REQUIRED_FOR_ENGINE006=NO` — utile (valide la machinerie d'override ciblé et la
méthodologie de fidélité) mais **non bloquant**, et son résultat ne doit **jamais** servir
d'estimation du domaine dominant `tq2_g128`.

## 5 — Réserve maintenue

```
REPLACEMENT_OR_ALTERNATE_KERNEL_PATH_FOR_DOMINANT_DOMAIN=DOES_NOT_EXIST
OLD_MMVQ_MMQ_BENCHMARKS=INSTRUCTIVE_NOT_PROOF   (MMQ indisponible pour le domaine dominant)
ONE_DISPATCH_SELECTION_EQUALS_ONE_LOGICAL_EXECUTION=NOT_PROVEN
```

## 6 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…   (identiques aux vierges)
Runtime de qualification : build-e5i (source vierge + census seul, zero evenement CUDA)
Instrument conservé : source-e5h/ggml-cuda-census.{cuh,cpp,cu}  (inchange)
Sequences rejouees : qualification/run003/run003-tokens.txt (fenetre 8 tokens)
```

## 7 — Ledger

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005D_STATUS=CLOSED_PROVENANCE
ENGINE005G_STATUS=CLOSED_ROLE_KEYED_RUNTIME_DISPATCH
ENGINE005H_STATUS=CLOSED_DISPATCH_DOMAIN
ENGINE005I_STATUS=GRAPH_ON_QUALIFICATION_DONE_SENSITIVITY_PENDING
ATTN_GATE_DOMINANT_DECODE_DOMAIN=TQ2_G128_MMVQ_ONLY
ATTN_GATE_ALTERNATE_MMQ_STRUCTURALLY_SUPPORTED=NO_FOR_DOMINANT_DOMAIN
ATTN_GATE_MMVQ_VS_MMQ_AB_ALLOWED=NO_AS_SPECIFIED
ATTN_GATE_MIXED_ROUTE_ROOT_CAUSE=DISJOINT_QTYPE_AND_SEQUENCE_DOMAINS
WHY_QUANTIZED_FFN_REACHES_OTHER=TQ2_G128_PREFILL_CAN_MMVQ_FALSE_AND_CAN_MMQ_FALSE
SSM_ALPHA_BETA_DOMINANT_KERNEL_PATH=MMVF_RUNTIME_PROVEN
DISPATCH_DECISION_DOMAIN_GRAPH_MODE_INDEPENDENT=YES
ATTN_GATE_DISPATCH_GRAPH_MODE_INDEPENDENT=YES
GRAPH_ON_SIGNATURE_ROWS=1738      GRAPH_OFF_SIGNATURE_ROWS=1738
SIGNATURE_SET_INTERSECTION=1738   MAPPING_DIVERGENT=0
GRAPH_ON_OUTPUT_WEIGHT_ROUTE=MMVQ
ENGINE006_CANDIDATE_SIGNATURE=recurrent_attn_gate.weight_tq2_g128_decode_s1_1_MMVQ
ENGINE006_CANDIDATE_ACTION=OPTIMIZE_CURRENT_MMVQ_FOR_TQ2_G128_DECODE
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
TARGET_MMVQ_CAUSAL_WALL_SENSITIVITY=NOT_MEASURED
NEXT_MEASUREMENT=NATIVE_MMVQ_DUPLICATION_R0_R1_R2
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
