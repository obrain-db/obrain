# ENGINE-005E — WALL-TIME CAUSAL BRIDGE : résultat

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5e/`

---

## 0 — Réponse

Premier pont mural causal établi, avec des portes qui ont **échoué** et d'autres qui ont **passé**.
Il faut lire les deux.

```
OUTPUT_PATH_WALL_COST_MS_PER_TOKEN = 5.25 ms
OUTPUT_PATH_WALL_PERCENT           = 8.24 %
```

sur une base rejouée de **63,87 ms/token** (ARM_LOGITS_ON, teacher-forced, runtime gelé).

**Mais** : la porte §7 échoue — `logits=false` **ne supprime pas** la projection de sortie. Le
delta mesure donc le chemin de sortie **quand les logits sont demandés**, pas l'économie d'un
calcul évitable.

---

## 1 — Protocole (§4 à §6)

Rejeu **teacher-forced** de la séquence gelée de RUN-003 (256 tokens identiques, injectés, jamais
choisis par les logits). Même modèle, même runtime (DSO gelées), même état initial, mêmes positions.

| | ARM_LOGITS_ON | ARM_LOGITS_OFF |
|---|---|---|
| `E5E_WALL_MS_PER_TOKEN` | **63,87** | **58,63** |
| `E5E_BACKEND_MS_PER_TOKEN` | 63,87 | 58,63 |
| `E5E_TPS` | 15,6568 | 17,0569 |
| `E5E_SYNC_WAIT_SUM_MS` (256 tok) | 16271,4 | 0,0068 |
| `E5E_HOST_SUBMISSION_SUM_MS` | 109,7 | 15036,2 |

Répétitions (runtime gelé, 3 par bras) — l'écart est **stable** :

```
ARM=ON   63,9888 / 63,8189 / 63,9867
ARM=OFF  58,7353 / 58,8050 / 58,8131
écart    5,25 / 5,01 / 5,17   ms par token
```

Les deux bras ont bien été **exécutés en intégralité** (les deux mesurent 256 décodages) ; ce qui
diffère est le point de synchronisation interne, pas le nombre de tokens traités :
avec logits, `llama_decode` revient en 0,43 ms et l'attente se fait au `get_logits` ; sans logits,
l'attente se fait dans `llama_decode`. Les deux se rejoignent sur la paroi totale.

## 2 — Porte §7 : ÉCHEC, et c'est le point important

```
LOGITS_FALSE_SKIPS_OUTPUT_PROJECTION=NO
```

Preuves :

1. **Lecture source** : `llama_context::graph_reserve` déclare `bool has_logits = true;` **sans
   condition** — la mémoire des logits est toujours dimensionnée, l'API n'a pas de mode « pas de
   tête ».
2. **Lecture source** (`src/models/qwen35.cpp`) : la construction du graphe fait toujours
   `cur = build_lora_mm(model.output, cur, model.output_s);` puis
   `ggml_build_forward_expand(gf, cur);` — le tenseur de sortie est toujours construit **et**
   toujours une cible du graphe. `batch.logits[i] == 0` ne pilote que la **copie vers l'hôte**
   (`if (logits.data && t_logits)` dans `llama-context.cpp`).
3. **Mesure** : `sched_reserve: graph nodes = 4011` **dans les deux bras**, à l'identique.
4. **Contre-épreuve d'implémentation** : un saut réel a été écrit et compilé (porte sur
   `inp_out_ids == nullptr` + variable d'environnement), et il **ne s'est jamais déclenché** —
   `build_inp_out_ids()` retourne **toujours** un tenseur (jamais `nullptr`), même avec
   `n_outputs == 0`. Le code de la bibliothèque le dit explicitement : la topologie du graphe est
   volontairement maintenue constante.
5. **Confirmation par les compteurs d'opérations** : le bras OFF présente **plus** de MUL_MAT
   (3138 vs 2562 sur la fenêtre), donc la tête n'a pas disparu.

**Conséquence honnête** : l'expérience ne répond pas à « combien coûte la projection de sortie si
on ne la calcule pas ». Elle répond à « combien coûte le chemin de sortie **lorsque les logits sont
demandés** ». Ce sont deux quantités différentes, et la seconde est celle du produit réel (un chat
demande toujours les logits pour échantillonner).

```
OUTPUT_WEIGHT_WALL_COST <= OUTPUT_PATH_WALL_COST = 5.25 ms/token
```

## 3 — Porte d'état : PASS

```
E5E_PROBE_LOGITS_SHA256=bebbfa0923f533d1134a030c502a58eb1ac93d2561ea43d80da9c701bfb4a996
   (identique dans les 6 exécutions, 3 ON + 3 OFF)
FINAL_MODEL_STATE_MATCH=YES   (périmètre : logits de sonde sur le token 198, tete active
                               dans les DEUX bras, apres rejeu des memes 256 tokens)
```

Périmètre déclaré : la comparaison porte sur les **logits d'un token de sonde unique** après le
rejeu complet. C'est un test d'équivalence d'état **fonctionnel** (tout écart de KV ou d'état
récurrent modifierait ces logits), pas un hachage du blob mémoire. Un hachage direct du cache KV
serait plus fort ; il n'a pas été fait.

## 4 — Delta causal primaire (§9, §10)

```
LOGITS_ON_BACKEND_ACTIVE_INTERVAL_SUM_MS  = 16351,4   (256 tokens)
LOGITS_OFF_BACKEND_ACTIVE_INTERVAL_SUM_MS = 15007,7   (256 tokens)
OUTPUT_PATH_WALL_COST_MS_PER_TOKEN        = (16351,4 - 15007,7) / 256 = 5,25 ms
OUTPUT_PATH_WALL_PERCENT                  = 5,25 / 63,87 = 8,24 %
```

Nature de ces 5,25 ms : elle **n'est pas** la copie hôte des logits (248 320 flottants ≈ 1 Mo
≈ 0,1 ms), et elle **n'est pas** le coût du MUL_MAT `output.weight` (il tourne dans les deux bras).
C'est donc du coût de **chemin de sortie** : écriture/copie GPU→hôte, synchronisation différentielle
et effets d'ordonnancement associés. Le fait que les deux bras atteignent la même paroi par des
chemins de synchronisation distincts est cohérent avec un coût **de pipeline**, pas de calcul.

## 5 — Confrontation au 17,27 % relatif de 005D

| | 005D (relatif) | 005E (mural) |
|---|---|---|
| `output.weight` / chemin de sortie | 17,27 % du temps MUL_MAT | **8,24 % de la paroi du token** |

Le premier pont est posé : une attribution **relative** devient une quantité **murale causale**.
Elle est **deux fois plus petite** en part de paroi qu'en part d'op-tuning — ce qui était attendu
(le temps MUL_MAT n'est pas le temps mural) et invalide définitivement toute conversion naïve
`part_op × paroi`.

## 6 — Ce que cela change pour ENGINE-006

Ce qui est **acquis** :

```
CAUSAL_WALL_IMPACT_IDENTIFIED=YES          (chemin de sortie : 5,25 ms/token, 8,24 %)
RUNTIME_PROVEN_KERNEL_PATH=NO              (toujours pas : les formes ne prouvent rien)
```

Ce qui reste **manquant** pour épingler une cible au sens de §20 :

1. `RUNTIME_PROVEN_KERNEL_PATH` — la route noyau réelle (MMVQ vs MMQ) pour `output.weight` et pour
   les familles `tq2_g128`. Cette passe ne l'a pas obtenue ; elle exige une instrumentation côté
   bibliothèque au point de sélection du noyau.
2. La **sensibilité** : 5,25 ms est le coût du chemin tel qu'il est. Rien ne dit qu'il est
   réductible — il faudrait un bras A/B de noyau équivalent (§16) ou une pente de travail dupliqué
   (§17).

```
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
ENGINE006_BRANCH=MATMUL_OPTIMIZATION
TOP_WALL_CANDIDATE=OUTPUT_PATH_5.25MS_PER_TOKEN_8.24_PERCENT
```

## 7 — Chemin correct proposé pour la suite

Puisque `logits=false` ne supprime rien, la mesure du **coût du calcul** de la tête demande une
intervention explicite, non un drapeau d'API :

- **bras A/B bibliothèque** (comme ici, mais avec un saut réellement actif) : porter la condition
  de saut sur ce qui est **réellement disponible** — par exemple un compteur d'itération du
  harnais lu par le builder, ou `n_outputs == 0` propagé jusqu'à `build()`, sans toucher la
  topologie par défaut. Un tel bras produit le coût **calcul** de la tête, séparé du coût
  **chemin de sortie** ci-dessus ;
- **route noyau** : instrumentation au point de sélection (`ggml_cuda_mul_mat` →
  `mul_mat_vec_q` / `mul_mat_q`), publiant un compteur par chemin, sans effet sur l'exécution.

## 8 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : ggml-cuda.cu (960e38d6…) et src/models/qwen35.cpp remis à l'identique
Runtimes de diagnostic : build-e4-observe (op-timing), build-e5c (provenance + saut de tete)
Séquence rejouée   : qualification/run003/run003-tokens.txt (gelée, 256 tokens)
```

Défauts rencontrés : harnais dépendant de symboles V3 absents du runtime op-timing (corrigé en
symboles faibles) ; deux tentatives de saut de tête sans effet (porte posée sur une condition
toujours vraie) — consignées, sans effet sur les chiffres publiés.

## 9 — Ledger

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005A_STATUS=CLOSED_GPU_TIMELINE
ENGINE005C_STATUS=CLOSED_METHOD_STRUCTURALLY_INSUFFICIENT
ENGINE005D_STATUS=CLOSED_PROVENANCE
EXECUTED_MUL_MAT_LAYER_COVERAGE_PERCENT=100.00
TOP_RELATIVE_MATMUL_SIGNATURE=output.weight_q6_K
TOP_RELATIVE_MATMUL_SIGNATURE_SHARE=17.27%
ENGINE005E_STATUS=CLOSED_WALL_BRIDGE_FIRST_QUANTITY
LOGITS_FALSE_SKIPS_OUTPUT_PROJECTION=NO
OUTPUT_PATH_WALL_COST_MS_PER_TOKEN=5.25
OUTPUT_PATH_WALL_PERCENT=8.24
LOGITS_ON_WALL_MS_PER_TOKEN=63.87
LOGITS_OFF_WALL_MS_PER_TOKEN=58.63
FINAL_MODEL_STATE_MATCH=YES_FUNCTIONAL_PROBE
CAUSAL_WALL_IMPACT_IDENTIFIED=YES
RUNTIME_PROVEN_KERNEL_PATH=NO
ABSOLUTE_OP_TIMING_TO_WALL_MAPPING=STILL_INVALID_FOR_NON_OUTPUT_FAMILIES
ENGINE006_BRANCH=MATMUL_OPTIMIZATION
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
NEXT_MEASUREMENT=REAL_HEAD_SKIP_ARM_PLUS_KERNEL_ROUTE_COUNTER
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
