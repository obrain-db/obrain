# ENGINE-004 / A″ — FORENSIC DE L'INVALIDATION DU CUDA GRAPH

Date : 2026-09-29. Machine : `spark-c578` (DGX, GB10, CUDA 13.2), alias ssh `dgx`.
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/`
Run canonique : `qualification/a2h/` (5 exécutions identiques, voir §7).

---

## 0 — La question posée

```
WHICH_PROPERTY_ACTUALLY_INVALIDATES_THE_GRAPH?
```

Réponse mesurée : **la forme (`ne`) du tenseur source du nœud 213 `cache_k_l3 (view)` change
lorsque la longueur de séquence du cache KV dépasse 1024 positions.** Rien d'autre.

---

## 1 — Statut acquis (inchangé)

```
A_TRANSITIONS=NO      B_TRANSITIONS=NO      C_TRANSITIONS=NO
ENGINE004_BOUNDARY_FORENSIC = CASE4_TRANSITION_NOT_REPRODUCED
RESET_ALONE_DID_NOT_TRIGGER_GRAPH_TRANSITION=YES
MEASUREMENT_ENTRY_PATH_DID_NOT_TRIGGER_GRAPH_TRANSITION=YES
NEXT_TOKEN_CONTINUITY_DID_NOT_TRIGGER_GRAPH_TRANSITION=YES
```

Correction de vocabulaire demandée et appliquée : `MEASURED_WINDOW_GRAPH_TRANSITION_EVENT_COUNT=0`
n'est **pas** publié (c'était faux), et `GRAPH_TRANSITION_EVENT_RATE=1/256` n'est **pas** publié
pour la fenêtre mesurée. Le `1` est classé dans `CUMULATIVE_PREWINDOW_COUNTER_STATE` (§5).

---

## 2 — Runtime observateur : provenance

| Élément | Valeur |
|---|---|
| `OBSERVER_BASE_GGML_CUDA_SOURCE_SHA256` | `960e38d680744e39723d241d4742856c1cb5336de13d007aa7a61debd5a67ada` |
| `OBSERVER_PATCH_SHA256` | `604ca922e2c519a59134330440e937627ed00a0a9ca90cf2ab2c9fe78f14deac` |
| `OBSERVER_FINAL_GGML_CUDA_SOURCE_SHA256` | `5bd0a258195bda877f44eb8a7dc1f84148943af66088d03bdbc9dd283bc566d3` |
| `OBSERVER_HEADER_SHA256` | `b9e7c4c1d4a14500135ae2b247110c9aa02a6fb82075edfeef885156e7ddc425` |
| `OBSERVER_LIBGGML_CUDA_SHA256` | `885c02392d0738d8d683583c4d0b9bf1c13f4b5688e320e32464885f75d882a1` |
| `OBSERVER_LIBLLAMA_SHA256` | `3db83675270ae68aa646aab65290fd3fb22a0e7491eaf9f49e58e8d52f74a802` (identique au gelé) |

Archives des deux fichiers d'état final (copies, SHA vérifiés) :
`engine-004-steady-state/RUN-001/source-final/ggml-cuda.cu` et `ggml-cuda-observe.cuh`.

```
OBSERVER_BASE = EXACT_FROZEN_ENGINE003_V3_SOURCE
OBSERVER_CONTAINS_ENGINE003_V3_COUNTER_PATCH = NO
OBSERVER_RUNTIME_CLASS = FORENSIC_ONLY_NONBASELINE_RUNTIME
```

**Preuve du choix de base.** La source figée engine-003 (`0cf69c28…`) a été comparée à l'arbre
vierge : la différence tient en **22 lignes de diff**, et ce sont **exactement** les lignes de
télémétrie V3 (bloc d'en-tête `engine002_v3_cuda_telemetry`, ses 9 sites `E2V3_INC`, et une ligne
vide). Il a donc été décidé de construire le runtime observateur **sur l'arbre vierge + patch
observateur additif**, plutôt que sur la source figée + patch : ce choix donne un runtime dont la
base est `960e38d6…`, vérifiable indépendamment, sans le mélange V3+observateur.

Conséquence assumée et déclarée : le runtime observateur **ne contient pas** la télémétrie V3. Le
harnais A″ déclare donc les symboles `engine002_v3_*` en `weak` et utilise la surface de
l'observateur. Le run A″ reste **byte-comparable** à A′ : la séquence de tokens produite est
identique (même SHA-256), ce qui est le seul juge de comparabilité (§8).

**Un incident de provenance est consigné** (§9) : un `make` lancé dans un répertoire de build
*copié* a écrit dans le répertoire d'origine (le cache CMake porte un répertoire binaire absolu).
La bibliothèque gelée `abafbf0d…` a été écrasée puis **restaurée à l'identique** depuis la copie
faite avant toute modification ; vérification finale : `sha256` = `abafbf0d…`, aucun symbole
d'observateur dans la bibliothèque servie.

```
OBSERVER_TPS = DIAGNOSTIC_ONLY
```

---

## 3 — Protocole A″ (§10)

Même chemin que A′ : contexte frais, même modèle, même préfixe 805, pré-roll adaptatif
jusqu'à deux `no_update_launch` consécutifs (obtenu en 4), puis **256 tokens** greedy.

| Contrainte | État mesuré |
|---|---|
| `NO_CSV_WRITES_INSIDE_WINDOW` | YES — tout écrit après la fenêtre |
| `NO_TEXT_LOGGING_INSIDE_WINDOW` | YES — `frozen_writes=0` |
| `NO_DYNAMIC_DIAGNOSTIC_ALLOCATION` | YES — buffers réservés avant |
| `OBSERVER_RING_PREALLOCATED` | YES — anneau statique de 4096 événements (1 048 576 octets réservés) |
| `PER_TOKEN_LIBRARY_TEXT_LOGGING` | **0** — 0 occurrence de `CUDA Graph id … reused` dans le stderr |
| lecture télémétrique par token | aucune — datation par **ordinal de calcul** |

---

## 4 — Réponses (§11)

```
A2_EVENT_COUNT_PREWINDOW=0
A2_EVENT_COUNT_MEASURED_WINDOW=2
A2_EVENT_COUNT_POSTWINDOW=0

A2_TRANSITION_COUNT_PREWINDOW=0
A2_TRANSITION_COUNT_MEASURED_WINDOW=1
A2_TRANSITION_COUNT_POSTWINDOW=0
A2_CAPTURE_COUNT_MEASURED_WINDOW=1

MEASURED_WINDOW_GRAPH_TRANSITION_EVENT_COUNT=1
```

Événement (le seul de la fenêtre) :

```
EVENT_0_COMPUTE_ORDINAL=224
EVENT_0_PHASE=MEASURED_WINDOW
EVENT_0_KIND=1
EVENT_0_CGRAPH_UID=39
EVENT_0_CACHED_UID=36
EVENT_0_N_NODES=4010
EVENT_0_FIRST_CHANGED_NODE_INDEX=213
EVENT_0_FIRST_CHANGED_NODE=cache_k_l3 (view)
EVENT_0_FIRST_CHANGED_FIELD=OTHER
EVENT_0_OLD_DATA_PTR=0 NEW_DATA_PTR=0
EVENT_0_OLD_NE=[256,4,1024,1] NEW_NE=[256,4,1280,1]
EVENT_0_OLD_NB=[2,512,2048,4194304] NEW_NB=[2,512,2048,4194304]
EVENT_0_OLD_BYTES=2097152 NEW_BYTES=2621440
WINDOW_BEGIN_COMPUTE_ORDINAL=9
WINDOW_END_COMPUTE_ORDINAL=265
```

### 4.1 Lecture

- La fenêtre mesurée couvre les ordinaux **9 → 264** (256 décodages).
- L'événement tombe à l'ordinal **224**, soit **216 tokens après l'ouverture de la fenêtre**.
- Nœud fautif : index **213** sur 4010, nom **`cache_k_l3 (view)`**.
- Le **pointeur de données ne bouge pas** (`old=new=0`).
- La **forme** change : `ne[2]` **1024 → 1280** ; les pas (`nb`) sont identiques ;
  les octets du nœud passent de **2 097 152** à **2 621 440** (+524 288 = +512 Ko).
- Interprétation : les tenseurs de cache KV sont des **vues** sur un tampon unique ; quand
  `n_ctx` effectif du cache passe de 1024 à 1280 positions, la vue `cache_k_l3` change de forme
  (et donc de taille) sans que l'adresse change. C'est **cette** signature qui est comparée par
  `ggml_cuda_graph_update_required`, donc c'est **elle** qui invalide le graphe.

`FIRST_CHANGED_FIELD=OTHER` est un résultat **honnête, pas un échec** : le champ fautif n'est ni
le pointeur, ni les pas, ni la forme d'une *source* — c'est le `ggml_tensor` du nœud lui-même
(vraisemblablement `view_off`/`view_src` de la vue, ou `ne[2]` du nœud). La nomenclature fermée
imposée par le protocole
(`UID | NODE_COUNT | NODE_TENSOR_BYTES | SRC_DATA_PTR | SRC_SHAPE_NE | SRC_STRIDE_NB | OTHER`)
ne contient pas de classe pour « métadonnée de vue » : `OTHER` est la classe correcte.
**Ce qui reste non identifié** : lequel des champs du nœud (au sens `ggml_tensor`) diffère
exactement. Les valeurs brutes publiées ci-dessus suffisent à caractériser la transition.

---

## 5 — Contradiction compteur / trace : RÉSOLUE

```
CUDA_COUNTER_TRANSITION_COUNT=1
OBSERVER_RING_TRANSITION_COUNT=1
DEBUG_TRACE_WARMUP_CYCLE_COUNT=2
DEBUG_TRACE_GRAPH_ID_COUNT=2
COUNTER_TRACE_CONSISTENCY=PASS
COUNTER_TRACE_CONTRADICTION=RESOLVED
```

Résolution, par ordinal :

| Trace | Ordinal | Nature |
|---|---|---|
| `warmup complete` (1er) + `warmup reset` (1er) | préfixe, **avant** l'ordinal 4 | transition logique **1** — pendant l'évaluation du préfixe (4 décodages de {256,256,245,48}) |
| `warmup complete` (2e) + graphe id 39 | **224** | transition logique **2** — dans la fenêtre mesurée |

Les deux messages/identifiants n'appartiennent donc **pas** à deux étapes d'une même transition
logique : ce sont **deux transitions logiques distinctes**, à deux ordinaux distincts. Le compteur
n'en voyait qu'une parce que la seconde — celle de la fenêtre — est la seule qu'il ait eu à
compter après la remise à zéro ; la première appartient à l'état cumulatif pré-fenêtre.

```
ONE_COUNTER_EVENT != ONE_COMPLETE_WARMUP_CYCLE   → REJETÉE (les deux cycles sont deux transitions)
CUMULATIVE_PREWINDOW_COUNTER_STATE = [pc=1 cb=1 ce=1 urT=1]   (transition du préfixe)
```

---

## 6 — Cas décisif (§12)

Un événement tombe **dans** les 256 tokens ⇒ la branche « transitions naturelles » s'applique :

```
NATURAL_LONG_RUN_GRAPH_TRANSITIONS_OBSERVED=YES
NATURAL_RUN_TRANSITION_RATE = 1 événement / 256 tokens
NATURAL_RUN_TRANSITION_ORDINAL = 224 (fenêtre 9..264)
```

Conséquence pour A′ : **A′ n'était pas stationnaire**, et la raison pour laquelle cela n'a pas été
vu est que les compteurs engine-003 sont cumulatifs et de remise à zéro ambiguë — le `pc=1 cb=1
ce=1 urT=1` lu en fin de fenêtre A′ était un état cumulatif, pas un comptage de fenêtre.
La conclusion « zéro transition dans la fenêtre » du rapport A′ est **retirée** : elle était fausse.

`ZERO_GRAPH_TRANSITIONS_OVER_256_TOKENS` est donc **abandonné comme exigence** (comme le prévoyait
le protocole), et remplacé par :

```
STEADY_STATE = LONG_RUN_STATISTICALLY_STABLE_REGIME
               INCLUDING_NORMAL_AMORTIZED_GRAPH_MAINTENANCE
```

La transition observée **n'est pas** un événement de bord de warmup : elle survient au milieu du
run, à un ordinal de calcul déterministe, déclenchée par la croissance du cache KV.

### 6.1 Réponses aux questions restées ouvertes depuis RUN-002

```
RUN2_GRAPH_KEY_CHANGED=MEASURED               (clé : premier nœud du graphe ; uid 36 → 39)
RUN2_CHANGED_GRAPH_PROPERTY=IDENTIFIED        (forme du nœud 213 cache_k_l3 (view), ne[2] 1024→1280)
BOUNDARY_TRANSITION_ROOT_CAUSE=IDENTIFIED     (voir §10 pour ce qui est établi et ce qui ne l'est pas)
```

---

## 7 — Reproductibilité

Cinq exécutions A″ (a2, a2b, a2c, a2e, a2h), trois versions successives de l'observateur :

| Run | ordinal | nœud | transitions fenêtre | séquence de tokens |
|---|---|---|---|---|
| a2 | 224 | 213 | 1 | identique à A′ |
| a2b | 224 | 213 | 1 | identique à A′ |
| a2c | 224 | 213 | 1 | identique à A′ |
| a2e | 224 | 213 | 1 | identique à A′ |
| a2h (canonique) | 224 | 213 | 1 | identique à A′ |

Même ordinal, même nœud, même comptage, même sortie : l'événement est **déterministe**, pas un
artefact.

---

## 8 — Fidélité (§14)

```
A2_GENERATED_TOKEN_COUNT=256
A2_OUTPUT_TOKEN_SEQUENCE_SHA256=210b7eb55ff64a10c1d51af5391edc21b8d4545d297817c332d5635842590231
APRIME_OUTPUT_TOKEN_SEQUENCE_SHA256=210b7eb55ff64a10c1d51af5391edc21b8d4545d297817c332d5635842590231
A2_TOKEN_SEQUENCE_MATCH_APRIME=YES
```

**Hard gate franchi.** L'observateur ne modifie ni le comportement ni la sortie : les 256 tokens
sont identiques à A′. Le run A″ est donc comparable, et les différences de latence ne peuvent pas
être attribuées à un changement de trajectoire.

---

## 9 — Incidents de provenance (consignés, non dissimulés)

1. **Répertoire de build copié, écriture dans l'original.** `cp -a` du répertoire de build puis
   `make` dans la copie : CMake avait enregistré un `CMAKE_BINARY_DIR` **absolu**, donc la
   compilation reliait et écrivait dans le répertoire de build d'origine d'engine-003. La
   bibliothèque `libggml-cuda.so.0.16.0` du répertoire de build engine-003 a été écrasée par la
   version observateur. Elle a été **restaurée à l'identique** depuis la copie
   `engine-004-steady-state/RUN-001/frozen-runtime/` faite **avant** toute modification
   (`abafbf0d…`), et la restauration est vérifiée (SHA + absence de symboles d'observateur).
2. **Trois exécutions invalidées par un build silencieusement non effectif.** Un patch appliqué à
   une source déjà patchee a produit un fichier incohérent ; `make` a échoué, mais les runs
   suivants ont réutilisé l'ancienne bibliothèque **sans le signaler** (SHA inchangé). Ces runs
   (`a2b`, `a2c`) ont été détectés par comparaison de SHA et **abandonnés**. Les portes dures
   (`MAKE_EXIT`, `SHA_LIB != SHA_PRECEDENTE`) ont été ajoutées aux scripts de reconstruction.
3. **Oubli de transmission du champ fautif.** Le champ classifié n'était pas passé à l'émission de
   l'événement (état local). Symptôme : `FIRST_CHANGED_FIELD=NONE` alors que le nœud et les
   valeurs étaient justes. Corrigé ; les runs antérieurs sont conservés, non retenus.

Aucun de ces incidents n'affecte les mesures retenues : chaque valeur publiée provient du run
`a2h`, dont la bibliothèque (`885c0239…`) et la séquence de tokens sont vérifiées.

---

## 10 — Décision de fermeture

```
A2_TOKEN_SEQUENCE_MATCH_APRIME=YES
COUNTER_TRACE_CONTRADICTION=RESOLVED
A2_EVENT_COUNT_MEASURED_WINDOW=1  (> 0)
ENGINE004_BOUNDARY_FORENSIC=COMPLETE
```

Ce qui est **établi** :

- la transition de graphe observée est provoquée par un **changement de forme** d'un tenseur de
  cache KV (`cache_k_l3`, `ne[2]` 1024→1280), sans changement de pointeur ni de pas ;
- elle est **déterministe** (même ordinal, 5/5) et **reproductible** ;
- la séquence générée est **inchangée** par la présence de l'observateur.

Ce qui **n'est pas** établi :

- que la transition de **RUN-002** était exactement celle-ci (RUN-002 a tourné avec les DSO gelées
  sans observateur : il faudrait le rejouer avec l'observateur pour l'affirmer) ;
- le champ exact du `ggml_tensor` de la vue qui diffère (`FIRST_CHANGED_FIELD=OTHER`) ;
- la fréquence des transitions sur des runs plus longs (une seule fenêtre de 256 tokens mesurée) :
  `GRAPH_TRANSITION_EVENT_RATE=1/256` est une mesure **ponctuelle**, pas un taux.

---

## 11 — RUN-003 (§17)

```
RUN003_RUNTIME=ENGINE003_FROZEN_V3_RUNTIME
libllama     = 3db83675270ae68aa646aab65290fd3fb22a0e7491eaf9f49e58e8d52f74a802
libggml-cuda = abafbf0dfe4f39d661401e0b481c7f90de547fa76a3647f51d2be9af790a7758
```

Les deux DSO sont restaurées et vérifiées. La condition de performance demandée est de fait
établie **en mieux** : la séquence de tokens produite sous observateur est identique à celle
produite sous le runtime gelé (même SHA-256), donc la non-perturbation comportementale est
mesurée, pas seulement affirmée.

```
OBSERVER_OVERHEAD_VS_APRIME_PCT = +1.35 %   (A2 15.6174 tps vs A′ 15.4092 tps ; DIAGNOSTIC ONLY)
```

Autorisation : la branche « transitions naturelles » est établie (§6), donc

```
ENGINE004_RUN3_ALLOWED=YES
```

avec `RUN003_RUNTIME=ENGINE003_FROZEN_V3_RUNTIME` (jamais le runtime observateur).

---

## 12 — Ledger

```
MODEL_RUN_COUNT=87
A_MICROTEST=NO_TRANSITION
B_MICROTEST=NO_TRANSITION
C_MICROTEST=NO_TRANSITION
ENGINE004_BOUNDARY_FORENSIC=COMPLETE
BOUNDARY_TRANSITION_ROOT_CAUSE=IDENTIFIED_KV_CACHE_SHAPE_GROWTH
RUN2_GRAPH_KEY_CHANGED=MEASURED
RUN2_CHANGED_GRAPH_PROPERTY=IDENTIFIED
CUDA_COUNTER_TRANSITION_COUNT=1
DEBUG_TRACE_WARMUP_CYCLE_COUNT=2
DEBUG_TRACE_GRAPH_ID_COUNT=2
COUNTER_TRACE_CONSISTENCY=PASS
COUNTER_TRACE_CONTRADICTION=RESOLVED
APRIME_COMPLETED=YES
A2_COMPLETED=YES
A2_TOKEN_SEQUENCE_MATCH_APRIME=YES
A2_EVENT_COUNT_PREWINDOW=0
A2_EVENT_COUNT_MEASURED_WINDOW=2
A2_EVENT_COUNT_POSTWINDOW=0
MEASURED_WINDOW_GRAPH_TRANSITION_EVENT_COUNT=1
NATURAL_LONG_RUN_GRAPH_TRANSITIONS_OBSERVED=YES
OBSERVER_BASE=EXACT_FROZEN_ENGINE003_V3_SOURCE
OBSERVER_CONTAINS_ENGINE003_V3_COUNTER_PATCH=NO
OBSERVER_RUNTIME_CLASS=FORENSIC_ONLY_NONBASELINE_RUNTIME
OBSERVER_TPS=DIAGNOSTIC_ONLY
PER_TOKEN_LIBRARY_TEXT_LOGGING=0
HARNESS_CPP_DEFECT_4_STATUS=RESOLVED
HARNESS_CPP_DEFECT_5_STATUS=RESOLVED
ENGINE004_BASELINE_FREEZE_ALLOWED=YES
RUN003_RUNTIME=ENGINE003_FROZEN_V3_RUNTIME
ENGINE004_RUN3_ALLOWED=YES
ENGINE005_ALLOWED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

`ENGINE004_BASELINE_FREEZE_ALLOWED` passe à YES : la contradiction compteur/trace qui le
bloquait est résolue (§5). `ENGINE005_ALLOWED` reste NO : hors périmètre de cette passe.
