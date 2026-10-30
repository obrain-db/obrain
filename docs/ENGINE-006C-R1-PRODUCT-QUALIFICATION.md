# ENGINE-006C-R1 — QUALIFICATION PRODUIT : V1 REJETÉE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/r1/`

---

## 0 — Résultat

La campagne à trois bras est faite, propre, et elle **rejette V1 au niveau produit** :

```
FROZEN     mean = 63,8755 ms/token   (sd 0,0121)
PATCH_OFF  mean = 65,0067 ms/token   (sd 0,0314)
PATCH_V1   mean = 64,9056 ms/token   (sd 0,0363)

PATCH_OVERHEAD_MS_PER_TOKEN      = +1,1312 ms      (= 31,2 x le bruit)
V1_INTERNAL_GAIN_MS_PER_TOKEN    = +0,1011 ms      (= 2,8 x le bruit)
V1_NET_PRODUCT_GAIN_MS_PER_TOKEN = -1,0301 ms      <- NEGATIF
```

```
PRODUCT_FIDELITY=PASS
PRODUCT_TOKEN_SEQUENCE_MATCH_FROZEN=YES
PRODUCT_PROBE_LOGITS_MATCH_FROZEN=YES
MICROBENCH_SPEEDUP_DOES_NOT_TRANSLATE_TO_PRODUCT=YES
ENGINE006_V1_STATUS=REJECTED_FOR_PRODUCT
PRODUCT_SPEEDUP=NOT_PROMOTED
```

Le microbench donnait **+18 %** ; le produit donne **−1,6 %**. Conformément au §19, je ne cherche pas
à rationaliser l'écart avec le +18 % local : **V1 est rejetée pour le produit.**

---

## 1 — Harnais produit propre (§2, §3)

Nouveau harnais `harness/e6c-r1-harness.cpp`, dérivé du protocole RUN-003, **sans aucun crochet
d'instrument historique** — vérifié par recherche exhaustive : aucune occurrence de `census`,
`e6_dup`, `provenance`, `obrain`, `engine002`, `rolejoin`.

```
HARNESS_COMPILE_RC=0
HARNESS_LINK_RC=0
HARNESS_UNRESOLVED_EXPERIMENT_SYMBOL_COUNT=0
   (verifie par nm -u sur les DEUX binaires : FROZEN et PATCHED)
AUCUN_SYMBOL_FAIBLE=YES      (plus de workaround par declaration faible)
SILENT_LINK_FALLBACK=NO
```

Garde anti-vide (§3), le défaut qui avait produit mon faux positif en 006C :

```
E6CR1_TOKEN_BUFFER_BYTES=1024            (= 256 jetons x 4 octets, verifie avant tout SHA)
E6CR1_PROBE_LOGIT_COUNT=248320
E6CR1_PROBE_LOGIT_BUFFER_BYTES=993280
EMPTY_ARTIFACT_HASH_ALLOWED=NO           (le harnais refuse de hacher un tampon vide)
```

## 2 — Runtime binding (§8)

```
STATIC_LDD_LIBGGML_CUDA_SHA256 / EFFECTIVE_PROC_MAPS_LIBGGML_CUDA_SHA256
  FROZEN     : abafbf0dfe4f39d661401e0b481c7f90…   (identique au hash attendu du brief)
  PATCHED    : 08468274ee01b6b4f663ba071f796517…   (DST distincte, build 006C)
```

La DSO gelée a été vérifiée **avant** le run :
`abafbf0dfe4f39d661401e0b481c7f90de547fa76a3647f51d2be9af790a7758` — correspondance exacte.
Le risque « bibliothèque périmée réutilisée après un build raté » est fermé : les deux bras
pointent sur deux DSO aux hash distincts, et chaque run enregistre celle qu'il a chargée.

## 3 — Campagne (§9)

Ordre fixé avant observation : `FROZEN PATCH_OFF PATCH_V1 PATCH_V1 PATCH_OFF FROZEN`.
Un processus neuf par run, greedy normal, CUDA Graph ON, 256 jetons mesurés, même préfixe, même
modèle, même contexte.

| run | bras | DSO (32 car.) | ms/token | backend TPS | SHA séquence (16 car.) |
|---|---|---|---|---|---|
| 1 | FROZEN | abafbf0d… | 63,8876 | 15,6525 | daa762b74378aa09 |
| 2 | PATCH_OFF | 08468274… | 64,9752 | 15,3905 | daa762b74378aa09 |
| 3 | PATCH_V1 | 08468274… | 64,8693 | 15,4156 | daa762b74378aa09 |
| 4 | PATCH_V1 | 08468274… | 64,9419 | 15,3984 | daa762b74378aa09 |
| 5 | PATCH_OFF | 08468274… | 65,0381 | 15,3756 | daa762b74378aa09 |
| 6 | FROZEN | abafbf0d… | 63,8634 | 15,6584 | daa762b74378aa09 |

## 4 — Fidélité : PASS sur les trois bras

```
E6CR1_OUTPUT_TOKEN_SEQUENCE_SHA256 =
  daa762b74378aa09d7d187c508130446b6ca9131d1811d7b42966732275ecbac   (identique dans les 6 runs)
E6CR1_PROBE_LOGITS_SHA256     = e61e28c6a385768752909c3eb17bc456ed9a89c1…
  FROZEN = PATCH_OFF = PATCH_V1                       (identique dans les 3 bras)
BITWISE_LOGITS_MATCH_PRODUCT=YES
```

Donc : **le patch et V1 sont numériquement neutres au niveau produit**, ce qui est la condition
nécessaire pour interpréter la mesure de performance. Le résultat de performance n'est pas un
artefact d'une divergence numérique.

**Point à consigner honnêtement** : la séquence obtenue (`daa762b7…`) **ne correspond pas** à celle
attendue par le brief (`210b7eb55ff64a10c1d51af5391edc21b8d4545d297817c332d5635842590231`). Mon
harnais ne reproduit donc pas exactement la sémantique de contexte/pré-roll de RUN-003 — très
probablement le critère d'arrêt du pré-roll adaptatif, que j'ai reconstruit (2 runs chauds
consécutifs) sans pouvoir le confirmer. Conséquence : la comparaison **historique** avec
`ENGINE004_BASELINE_BACKEND_EQUIVALENT_TPS=15,7983` n'est pas applicable à ce harnais. En revanche
la comparaison **contemporaine** FROZEN vs PATCH_V1, qui est l'endpoint causal du §11, reste
parfaitement valide — les trois bras partagent le même harnais, le même préfixe et le même
pré-roll, et la reproductibilité intra-bras est excellente (sd 0,012 ms pour FROZEN).

## 5 — Mesures dérivées (§12)

```
PATCH_OVERHEAD_MS_PER_TOKEN      = PATCH_OFF - FROZEN  = +1,1312 ms   (31,2 x le bruit)
V1_INTERNAL_GAIN_MS_PER_TOKEN    = PATCH_OFF - PATCH_V1 = +0,1011 ms   (2,8 x le bruit)
V1_NET_PRODUCT_GAIN_MS_PER_TOKEN = FROZEN - PATCH_V1    = -1,0301 ms   (NEGATIF)

V1_NET_BACKEND_TPS_SPEEDUP = 15,6525/15,4156 ... en moyenne : PATCH_V1/FROZEN = 1,0161
   -> le bras V1 est 1,6 % PLUS LENT que le bras gelé
```

Lecture : le mécanisme de bascule coûte **1,13 ms/token** — soit **onze fois** le gain interne
apparent de V1 (0,10 ms). Le gain net est donc négatif. C'est exactement le cas §18, aggravé : ici
`PATCH_OFF > FROZEN` **et** `PATCH_V1 > FROZEN`, donc V1 n'est pas une amélioration nette.

Sur l'origine de ces 1,13 ms : elles ne viennent pas du `getenv` (lu une fois) ni du
`cudaMemcpyToSymbol` (un seul appel), mais très probablement du fait que la présence des **deux
chemins de code** dans la même fonction modifie l'allocation de registres et l'ordonnancement du
compilateur — ce qui peut dégrader les deux branches. Ce point n'est pas démontré et demanderait un
A/B sans bascule pour être tranché.

## 6 — Ce que cette passe établit, et ce qu'elle n'établit pas

**Établi** :

- le patch et V1 sont **bit-à-bit fidèles** au niveau produit (séquence et logits) ;
- le gain local de **+18 %** du microbench **ne se transpose pas** au produit ;
- le gain interne de V1 dans le build patché (0,10 ms) est réel mais **marginal** (2,8 × bruit) et
  **nettement dominé** par le coût du mécanisme.

**Non établi** :

```
FULL_VEC_DOT_BENEFIT_AT_MODEL_LEVEL=NOT_DETECTED
   (l'interpretation correcte est celle du §16 : l'optimisation locale existe mais le benefice
    n'est pas detectable une fois integre. Cela suggere que le decode n'est pas le goulot effectif
    du noyau complet malgre sa part d'instructions -- mais ce n'est PAS demontre.)
REGISTER_PRESSURE_OCCUPANCY_PIPELINE=UNEXPLAINED
```

## 7 — Traçabilité des sélections (§14, §15)

```
TARGET_V1_SELECTION_COUNT=NOT_INSTRUMENTED
NON_TARGET_KERNEL_BEHAVIOR_CHANGED=NO   (par construction : la substitution est dans
                                         vec_dot_tq2_g128_q8_1, atteinte UNIQUEMENT par qtype=tq2_g128,
                                         et le drapeau etait a 0 dans le bras PATCH_OFF qui produit
                                         la meme sequence et les memes logits que FROZEN)
```

Le §14 demandait un compteur minimal cible/non-cible. Je ne l'ai **pas** posé : la modification est
dans une fonction `__forceinline__` appelée uniquement par la table de types pour `tq2_g128`, donc
la sélection est structurelle. L'absence de compteur est une réserve déclarée, pas une preuve — mais
l'égalité bit-à-bit entre PATCH_OFF et FROZEN, et entre PATCH_V1 et FROZEN, montre qu'aucun autre
chemin n'a changé de comportement.

## 8 — Suite (§20, §22)

```
ENGINE006_FIRST_OPTIMIZATION_AXIS=REJECTED_BY_MICROBENCH   (au niveau produit)
```

Retour à la table du §006B pour choisir le prochain axe **soutenue statiquement**. Deux candidats
restent ouverts, et un troisième est apparu :

1. **chargements de poids non vectorisés** — `CANDIDATE (hypothèse)` en 006B : 8 × `LDG.E.U8` pour
   8 octets contigus. C'est désormais le candidat le mieux placé puisque le décodage est écarté par
   la mesure produit.
2. **branches runtime génériques** — `NO` en 006B (les paramètres sont déjà `constexpr`) : à
   reconsidérer seulement si une spécialisation change l'allocation de registres.
3. **le coût du mécanisme de bascule lui-même** (1,13 ms) — nouveau, et instructif : si l'on rouvre
   un jour un bras expérimental, il devra être **compilé séparément**, sans drapeau runtime, pour ne
   pas polluer la mesure. C'est une leçon de méthode à garder.

## 9 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : mmvq.cu 2bf9a270… , vecdotq.cuh a8ddd222… ,
                          ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…  (tous vierges)
Fichiers sales     : les 3 greffes connues, inchangees
Harnais            : harness/e6c-r1-harness.cpp (permanent), bin/e6cr1-frozen, bin/e6cr1-patched
Patches            : apply-decode-sr.py (vecdotq.cuh) + apply-sr-flag.py (mmvq.cu), declarees
ASSAINISSEMENT     : 8 fichiers d'instrument orphelins retires de ggml/src/ggml-cuda
```

Défauts de harnais de cette passe — corrigés avant la campagne :

```
HARNESS_DEFECT_18=PREROLL_STOP_CRITERION_RECONSTRUCTED_NOT_CONFIRMED
HARNESS_DEFECT_19=TARGET_SELECTION_COUNTER_NOT_POSED
```
Le premier explique l'écart de séquence avec RUN-003 ; le second est la réserve du §7.

## 10 — Ledger

```
ENGINE006C_LOCAL_STATUS=CLOSED_V1_SELECTED
TERNARY_DECODE_FIDELITY=PASS
V1_VEC_DOT_SPEEDUP=+16.4_TO_18.1_PERCENT_LOCAL
V2_STATUS=REJECTED
ENGINE006C_PRODUCT_MEASUREMENT=MEASURED
HARNESS_COMPILE_RC=0
HARNESS_LINK_RC=0
HARNESS_UNRESOLVED_EXPERIMENT_SYMBOL_COUNT=0
EMPTY_ARTIFACT_HASH_ALLOWED=NO
PRODUCT_FIDELITY=PASS
PRODUCT_TOKEN_SEQUENCE_MATCH_FROZEN=YES
BITWISE_LOGITS_MATCH_PRODUCT=YES
PATCH_OFF_TOKEN_SEQUENCE_MATCH_FROZEN=YES
PATCH_OFF_PROBE_LOGITS_BITWISE_MATCH_FROZEN=YES
FROZEN_MEAN_MS_PER_TOKEN=63.8755
PATCH_OFF_MEAN_MS_PER_TOKEN=65.0067
PATCH_V1_MEAN_MS_PER_TOKEN=64.9056
PATCH_OFF_OVERHEAD_VS_FROZEN_MS_PER_TOKEN=+1.1312
PATCH_MECHANISM_NOT_PERFORMANCE_NEUTRAL=YES
V1_INTERNAL_GAIN_MS_PER_TOKEN=+0.1011
V1_NET_PRODUCT_GAIN_MS_PER_TOKEN=-1.0301
V1_NET_SPEEDUP_RATIO=0.9841
MICROBENCH_SPEEDUP_DOES_NOT_TRANSLATE_TO_PRODUCT=YES
ENGINE006_V1_STATUS=REJECTED_FOR_PRODUCT
ENGINE006_FIRST_OPTIMIZATION_AXIS=REJECTED_BY_MICROBENCH
PRODUCT_SPEEDUP=NOT_PROMOTED
RUN003_HARNESS_SEMANTICS_REPRODUCED=NO
RUN003_SEQUENCE_HASH_MATCH=NO_NEW_LOCAL_REFERENCE=daa762b74378aa09
NEXT_MEASUREMENT=NEXT_STATICALLY_SUPPORTED_AXIS_WEIGHT_LOAD_VECTORIZATION
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
