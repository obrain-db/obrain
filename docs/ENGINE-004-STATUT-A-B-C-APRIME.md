# ENGINE-004 — STATUT CORRIGÉ (A/B/C + A′)

Date : 2026-09-29. Machine d'exécution : `spark-c578` (DGX, GB10, CUDA 13.2), alias ssh `dgx`.
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/`

---

## 1 — Correction du statut A/B/C

Le bloc A n'était pas tronqué : **le harnais A n'avait jamais produit de log conservé**. Le seul
`stdout` présent (`qualification/stdout.txt`) ne contient que les sorties de `steady256` ; aucun
`A_DELTA_*` n'y figure. A a donc été **ré-exécuté** (un seul token de test, comme prévu), avec le
binaire `bin/boundary-abc` et les bibliothèques instrumentées engine-003.

```
ENGINE004_BOUNDARY_FORENSIC = CASE4_TRANSITION_NOT_REPRODUCED
A_TRANSITIONS=NO
B_TRANSITIONS=NO
C_TRANSITIONS=NO
BOUNDARY_TRANSITION_ROOT_CAUSE = NOT_ESTABLISHED
```

`A_TRANSITIONS=NO` n'est plus `NOT_CONFIRMED` : il est **mesuré** (voir §2).

---

## 2 — A : ré-exécution, deltas publiés

Commande exacte :

```sh
cd /home/triviere/exp-artifacts/engine-004-steady-state/RUN-001
./bin/boundary-abc \
  /home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf \
  /home/triviere/exp-artifacts/engine-001-qwen3-8-27b-state-native-bench/RUN-001/qualification/prefix805.ids.txt \
  A
```

Sortie (fichiers `qualification/apr-a/stdout-A.txt`, `stderr-A.txt`) :

```
MODE=A ADAPTIVE_PREROLL=4 CONSEC_NO_UPDATE=2
PRECOND_CUM[cb=1 ce=1 inst=1 upd=0 pc=1 urT=1 urF=3 lch=3 nol=2]

A_DELTA_PROPERTIES_CHANGED=0
A_DELTA_UPDATE_REQUIRED_TRUE=0
A_DELTA_UPDATE_REQUIRED_FALSE=1
A_DELTA_CAPTURE_BEGIN=0
A_DELTA_CAPTURE_END=0
A_DELTA_INSTANTIATE=0
A_DELTA_EXEC_UPDATE=0
A_DELTA_LAUNCH=1
A_DELTA_NO_UPDATE_LAUNCH=1
A_TRANSITIONS=NO
```

Exactement le profil attendu par le protocole (tous les deltas de transition à 0, un seul launch
en régime no-update). Le pré-roll adaptatif montre la transition unique du warmup
(`pc=1 urT=1 cb=1 ce=1 inst=1`) puis la saturation (`nol=2`).

Pièce de provenance recoupée : le stderr contient `CUDA Graph id 36 reused` (chemin rapide UID),
conforme à l'observation déjà consignée.

---

## 3 — Conservé tel quel (ne pas généraliser)

```
TELEMETRY_RESET_NOT_CAUSAL=SUPPORTED
MEASUREMENT_ENTRY_PATH_NOT_CAUSAL=SUPPORTED
RESET_ALONE_DID_NOT_TRIGGER_GRAPH_TRANSITION=YES
RUN2_MEASUREMENT_ENTRY_SEQUENCE_DID_NOT_TRIGGER_GRAPH_TRANSITION_IN_MICROTEST=YES
```

Portée : un token de test, un pré-roll adaptatif à 2 `no_update_launch` consécutifs, modèle
Qwen3.8-27B-OBRAIN-v3, libs engine-003. Rien de plus.

```
CUDA_GRAPH_UID_FAST_PATH_SOURCE_PROVEN=YES
CUDA_GRAPH_UID_FAST_PATH_RUNTIME_OBSERVED=YES
RUN2_GRAPH_KEY_CHANGED=NOT_MEASURED
RUN2_CHANGED_GRAPH_PROPERTY=NOT_IDENTIFIED
```

---

## 4 — A′ : long-run naturel minimal

Programme : `source/aprime.cpp` (harnais), `bin/aprime` (binaire), sorties dans
`qualification/aprime/`. Même modèle, même préfixe 805, même chemin greedy autorégressif,
contexte frais, pré-roll adaptatif identique, puis **256 tokens**.

Discipline de fenêtre :

| Contrainte | État |
|---|---|
| `NO_CSV_WRITES` | YES — tous les fichiers écrits après la fermeture de la fenêtre |
| `NO_PER_TOKEN_TEXT_LOGGING` | YES (sortie harnais) — mais voir la réserve §6 |
| `NO_DYNAMIC_ALLOCATION_FOR_DIAGNOSTICS` | YES — buffers `vector` réservés avant la fenêtre |
| `NO_RUNTIME_PATCH` | YES — API publique uniquement |
| télémétrie | reset **une fois** avant, lecture **une fois** après (aucune lecture CUDA par token) |

Résultats :

```
APRIME_ADAPTIVE_PREROLL=4
PREROLL_CUDA[cb=1 ce=1 inst=1 upd=0 pc=1 urT=1 urF=3 lch=3 nol=2 nul=0]
WARMUP_CAPTURE_INVARIANT=PASS

APRIME_GENERATED_TOKEN_COUNT=256
APRIME_OUTPUT_TOKEN_SEQUENCE_SHA256=210b7eb55ff64a10c1d51af5391edc21b8d4545d297817c332d5635842590231
APRIME_WINDOW_WALL_S=16.613425
APRIME_TELEMETRY_READ_MS=0.0014

APRIME_PROPERTIES_CHANGED_TRUE=1
APRIME_UPDATE_REQUIRED_TRUE=1
APRIME_UPDATE_REQUIRED_FALSE=255
APRIME_INSTANCE_NULL_TRUE=0
APRIME_CAPTURE_BEGIN=1
APRIME_CAPTURE_END=1
APRIME_INSTANTIATE=0
APRIME_EXEC_UPDATE=0
APRIME_LAUNCH=255
APRIME_NO_UPDATE_LAUNCH=254
APRIME_CAPTURE_BEGIN_EQ_CAPTURE_END=PASS
```

### 4.1 Interprétation

Le SHA-256 de la séquence produite est **identique** à celui de RUN-002
(`210b7eb5…90231`) : A′ reproduit **le même run** que RUN-002, instrumentation en moins.

La fenêtre de 256 tokens contient **une** transition de graphe, pas zéro :

```
GRAPH_TRANSITION_EVENT_COUNT=1
GRAPH_TRANSITION_EVENT_RATE_PER_TOKEN=0.00390625
```

Cette transition est datée (§5) **avant 500 calculs**, c'est-à-dire **avant la fenêtre mesurée**.
Répartition dans la fenêtre : un token en capture+instanciation (propriétés changées), puis
255 tokens en chemin rapide, dont 254 `no_update_launch`.

Précision de vocabulaire, importante : les compteurs lus sont **cumulatifs** (la remise à zéro
n'est pas neutre pour eux, cf. §6). `APRIME_CAPTURE_BEGIN=1` ne veut donc pas dire « 1 transition
parmi 256 tokens », mais « le compteur vaut 1 au moment de la lecture, la transition ayant eu
lieu avant la fenêtre ». Le décompte des transitions **dans** la fenêtre vient du log de warmup et
de la date de la bascule (§5), pas du compteur.

Conséquence pour le protocole : l'absence de transition sur un token A/B/C ne suffisait pas à
caractériser un long run, **mais** dans ce protocole-ci les transitions naturelles sont
**précoces** (warmup puis re-warmup), pas distribuées dans le temps. Après la seconde
stabilisation, la fenêtre est stationnaire au sens statistique.

```
NATURAL_LONG_RUN_GRAPH_TRANSITIONS_OBSERVED=NO
  (au sens du critère du protocole : pc>0 / cb>0 / urT>0 DANS la fenêtre mesurée = 0)
NATURAL_LONG_RUN_TRANSITION_OCCURRED_OUTSIDE_WINDOW=YES
RUN003_MODE=STRICT_NO_TRANSITION (candidat, non autorisé à ce stade — voir §8)
ZERO_GRAPH_TRANSITIONS_OVER_256_TOKENS : NON abandonné comme exigence, mais reformulé en
  ZERO_GRAPH_TRANSITIONS_INSIDE_THE_MEASURED_WINDOW (vrai), la transition de re-warmup
  restant un événement de bord antérieur à la fenêtre.
```

Deux points explicitement **non mesurés** à ce stade :

- `NO_UPDATE_TOKEN_LATENCY_DISTRIBUTION` : disponible (255 tokens sans transition) —
  voir §7, colonne `APRIME_NO_UPDATE_LAUNCH=254`. Il faut toutefois noter que sur ces 255 tokens,
  254 seulement sont des no-update stricts.
- `TRANSITION_TOKEN_LATENCY_DISTRIBUTION` : **non mesurable ici** — il n'y a qu'un seul token de
  transition dans la fenêtre, une distribution sur un point n'existe pas.

---

## 5 — Preuve de datation (événement silencieux)

Le log stderr de A′ contient **deux** identifiants de graphe et **deux** cycles de warmup :

```
1282: CUDA Graph id 36 reused          (218 occurrences de l'id 36)
1283: ggml_backend_cuda_graph_compute: CUDA graph warmup complete
1501: ggml_backend_cuda_graph_compute: CUDA graph warmup reset
1502: CUDA Graph id 39 reused          (40 occurrences de l'id 39)
1503: ggml_backend_cuda_graph_compute: CUDA graph warmup complete
```

Le compteur engine-003 ne voit qu'**une** transition (`pc=1`) et **zéro** instanciation dans la
fenêtre. Écart **mesuré**, non expliqué : les compteurs et les traces de warmup ne racontent pas
la même chose.

Datation par borne explicite (harnais `aprime2.cpp` : imprime un marqueur au N-ième calcul, sans
aucun appel d'horloge tant que la borne n'est pas atteinte) :

| Budget (calculs) | Marqueur atteint | Bascule 36→39 avant le marqueur | Fenêtre terminée |
|---|---|---|---|
| 500 | non | — | oui (256 tokens, même SHA) |
| 760 | non | — | oui |
| 812 | non | — | oui |

Donc : **la seconde transition et le passage à l'id 39 se produisent avant 500 calculs**, soit
pendant le préfixe/le pré-roll — **avant** les 256 tokens mesurés. Aucun appel d'horloge n'a été
ajouté dans la fenêtre pour l'établir.

---

## 6 — Réserve de discipline (à corriger au prochain run)

`NO_PER_TOKEN_TEXT_LOGGING=YES` vaut pour le harnais, **pas pour la bibliothèque** : le chemin
rapide UID fait un `GGML_LOG_DEBUG("CUDA Graph id %zu reused\n")` par token. Le stderr de A′
contient 218 + 40 lignes de ce type (1544 lignes au total, 76 ko).

Conséquence : la fenêtre A′ n'est **pas** libre d'I/O de diagnostic. C'est une limite explicite du
résultat. Pour A″, il faut soit compiler la bibliothèque sans ce log (niveau debug retiré), soit
rediriger stderr vers `/dev/null` — dans les deux cas, la variante doit être documentée.

---

## 7 — Défauts 4/5 du harnais : RÉSOLUS

Voir `docs/ENGINE-004-DEFAUTS-4-5.md`. Attribution mesurée dans A′ (256 tokens, un seul run) :

| Métrique | p50 (ms) | mean (ms) | total (ms) |
|---|---|---|---|
| `HOST_DECODE_SUBMISSION_MS` (t1−t0) | 0.070 | 0.210 | 53.784 |
| `BACKEND_SYNC_WAIT_MS` (t2−t1) | 64.092 | 64.015 | 16387.798 |
| `DECODE_PLUS_SYNC_MS` (t2−t0) | 64.165 | 64.225 | 16441.582 |
| `ARGMAX_MS` (t3−t2) | 0.671 | 0.671 | 171.836 |
| `GREEDY_TOKEN_MS` (t3−t0) | — | 64.90 | 16613.4 |

```
BACKEND_ACTIVE_INTERVAL_SUM_MS=16441.5816
BACKEND_EQUIVALENT_TPS=15.5703
GREEDY_CONTINUOUS_WALL_SECONDS=16.613425
END_TO_END_GREEDY_TPS=15.4092
```

Conclusion d'attribution : **99,7 % du temps par token est l'attente de synchronisation du
backend**, pas la soumission hôte (0,07 ms) ni l'argmax (0,67 ms). Les deux défauts sont résolus
au sens où les deux grandeurs sont désormais séparées et publiées.

Point de vigilance : `HOST_DECODE_SUBMISSION_MS` a un maximum à 27,976 ms (un décrochage
d'ordonnancement), qui explique aussi le `max` de `DECODE_PLUS_SYNC_MS` (72,418 ms).

---

## 7bis — Observateur de transitions CUDA Graph (préparé, instrumenté, compilé)

La question restée ouverte — `RUN2_CHANGED_GRAPH_PROPERTY=NOT_IDENTIFIED` — demande de nommer
**quel** nœud change de propriété et **quel** champ change. Les compteurs engine-003 en sont
incapables (ils comptent, ils ne nomment pas).

Livré :

- `engine-004-aprime/source/ggml-cuda-observe.cuh` — observateur : compteurs exacts
  (`properties_changed_true`, `update_required_true`, `capture_begin`, `capture_end`,
  `instantiate`, `exec_update`, `exec_update_reinstantiate`, `launch`, `no_update_launch`,
  `warmup_reset`, `uid_reused`), **plus** un anneau d'événements borné (512) portant
  `uid`, index et **nom** du premier nœud fautif, le **champ** fautif, et la valeur de pointeur
  source. Aucune allocation dynamique, aucun ordonnancement, aucune I/O dans le chemin chaud.
- `engine-004-aprime/source/apply-observer.py` — application **auto-vérifiante** du patch à
  `ggml-cuda.cu` : chaque ancre doit apparaître exactement une fois, sinon rien n'est écrit
  (pas de patch partiel silencieux). 11 points d'insertion, les 9 ancres vérifiées.
- Freeze barrière non déclarative : `ggml_obrain_obs_freeze(true)` ferme l'écriture
  d'observation ; toute tentative pendant la fenêtre incrémente `frozen_writes` et fait échouer
  l'invariant `INVARIANT_NO_IO_DURING_WINDOW=OK`.

État : patch appliqué à l'arbre **DGX** `/home/triviere/projects/llama-obrain-cuda`
(fichier modifié, **non commité**), compilation ciblée dans
`build-e4-observe/` — les bibliothèques engine-003 servies par `boundary-abc`, `steady256b` et
`aprime` **n'ont pas été touchées** (comparabilité des mesures préservée).

Vérifications faites :

- l'ancien `ggml-cuda.cu` a été **restauré à l'identique** avant application
  (SHA-256 `960e38d6…e5a67ada`, `git status` propre), puis patché par le script ;
- `make -j14 ggml-cuda` : exit 0, aucune erreur ; l'objet contient les 12 appels
  (`grep -c ggml_obrain_obs` = 12) ;
- l'objet contient bien le vidage (`ggml_obrain_obs_dump`, `ggml_obrain_obs_install_atexit`)
  et la chaîne `INVARIANT_CAPTURE_BEGIN_EQ_CAPTURE_END`.

Premier défaut corrigé en cours de route : sans l'appel explicite
`ggml_obrain_obs_install_atexit()`, l'éditeur de liens **éliminait le vidage** (fonction inline
jamais référencée). Le compteur aurait été juste et muet. C'est exactement le genre de défaillance
silencieuse que ce protocole doit interdire : la vérification se fait maintenant sur l'objet
compilé, pas sur la source.

Variables d'environnement : `GGML_OBSERVE=1` (arme), `GGML_OBSERVE_OUT=<chemin>` (vidage en sortie
de processus), `GGML_OBSERVE_VERBOSE=1` (trace texte, à n'utiliser QUE hors fenêtre).

Ce que cela débloque au prochain run : le report exact de
`RUN2_CHANGED_GRAPH_PROPERTY` (nom du nœud + champ) et la distinction entre
« même clé de graphe, propriétés changées » et « clé de graphe changée ».

---

## 8 — État de l'autorisation RUN-003

| Condition | État |
|---|---|
| `A_MICROTEST=CONFIRMED` | **YES** (ré-exécuté, §2) |
| `HARNESS_CPP_DEFECT_4_STATUS=RESOLVED` | **YES** (§7) |
| `HARNESS_CPP_DEFECT_5_STATUS=RESOLVED` | **YES** (§7) |
| `APRIME_COMPLETED=YES` | **YES** (§4) |
| `NATURAL_LONG_RUN_GRAPH_TRANSITIONS_OBSERVED=YES` | NO |
| `RUN2_TRANSITION_MECHANISM_EXPERIMENTALLY_AVOIDED_OR_RESOLVED=YES` | NO |

Lecture stricte : `NO OR NO` ⇒ **`ENGINE004_RUN3_ALLOWED=NO`**. La branche « transitions
naturelles » n'est pas établie *dans la fenêtre*, et la branche « mécanisme RUN-002 évité ou
résolu » ne l'est pas non plus (la cause racine reste `NOT_ESTABLISHED`).

Recommandation : **A″ d'abord** (mêmes tokens, mêmes rebouchages en mémoire, écriture après la
fenêtre), puis A‴ seulement si A″ est stable. Le mode `STRICT_NO_TRANSITION` reste un candidat,
pas un acquis.

---

## 9 — Ledger corrigé

```
MODEL_RUN_COUNT=87
ENGINE004_RUN1_STATUS=MEASURED_NONSTRICT_GRAPH_WINDOW
ENGINE004_RUN2_STATUS=MEASURED_NONSTRICT_GRAPH_WINDOW
A_MICROTEST=CONFIRMED_NO_TRANSITION
A_TRANSITIONS=NO
B_MICROTEST=NO_TRANSITION
C_MICROTEST=NO_TRANSITION
ENGINE004_BOUNDARY_FORENSIC=CASE4_TRANSITION_NOT_REPRODUCED
TELEMETRY_RESET_NOT_CAUSAL=SUPPORTED
MEASUREMENT_ENTRY_PATH_NOT_CAUSAL=SUPPORTED
RESET_ALONE_DID_NOT_TRIGGER_GRAPH_TRANSITION=YES
RUN2_MEASUREMENT_ENTRY_SEQUENCE_DID_NOT_TRIGGER_GRAPH_TRANSITION_IN_MICROTEST=YES
RUN2_GRAPH_PROPERTY_TRANSITION=MEASURED
RUN2_GRAPH_KEY_CHANGED=NOT_MEASURED
RUN2_CHANGED_GRAPH_PROPERTY=NOT_IDENTIFIED
BOUNDARY_TRANSITION_ROOT_CAUSE=NOT_ESTABLISHED
CUDA_GRAPH_UID_FAST_PATH_SOURCE_PROVEN=YES
CUDA_GRAPH_UID_FAST_PATH_RUNTIME_OBSERVED=YES
APRIME_COMPLETED=YES
NATURAL_LONG_RUN_GRAPH_TRANSITIONS_OBSERVED=NO
NATURAL_LONG_RUN_TRANSITION_OCCURRED_OUTSIDE_WINDOW=YES
GRAPH_TRANSITION_EVENT_COUNT=1
GRAPH_TRANSITION_EVENT_RATE_PER_TOKEN=0.00390625
APRIME_TRANSITION_DATED_BEFORE_COMPUTE_INDEX=500
HARNESS_CPP_DEFECT_4_STATUS=RESOLVED
HARNESS_CPP_DEFECT_5_STATUS=RESOLVED
APRIME_WINDOW_FREE_OF_DIAGNOSTIC_IO=NO (log debug bibliothèque, voir §6)
ENGINE004_STATUS=LONG_RUN_BOUNDARY_FORENSIC_IN_PROGRESS_APRIME_DONE
ENGINE004_RUN3_ALLOWED=NO
ENGINE005_ALLOWED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
