# ENGINE-004 — DÉFAUTS 4 ET 5 DU HARNAIS C++ : SPÉCIFICATION RÉSOLUE

Statut : `HARNESS_CPP_DEFECT_4_STATUS=RESOLVED`, `HARNESS_CPP_DEFECT_5_STATUS=RESOLVED`
Preuve d'exécution : `qualification/aprime/stdout-aprime.txt` (256 tokens, DGX spark-c578).

---

## 1 — Le défaut

Sur RUN-001 et RUN-002, la latence par token était attribuée à des enveloppes qui ne
correspondaient pas aux phases réelles :

| Run | `DECODE_CALL_MS` | `GETLOGITS_MS` | Lecture |
|---|---|---|---|
| RUN-001 (`steady256`) | p50 = 0,067 ms | p50 = 63,539 ms | le coût était attribué à la *récupération des logits* |
| RUN-002 (`steady256b`) | p50 = 64,020 ms | p50 = 0,001 ms | le même coût était attribué à *llama_decode* |

Les deux runs mesurent le **même modèle, le même préfixe, le même chemin greedy** et produisent
la **même séquence** (SHA-256 `210b7eb5…90231` pour RUN-002 et pour A′). Une grandeur physique
identique ne peut pas changer de camp entre deux runs : c'était un artefact de placement des
appels `clock_gettime`, pas un changement de comportement.

C'est le défaut 4 (attribution) et le défaut 5 (grandeurs manquantes : soumission hôte vs attente
de synchronisation, TPS backend vs TPS greedy).

---

## 2 — Les quatre points d'horodatage

Dans le harnais (`aprime.cpp`, lambda `step`), autour du pas greedy complet :

```cpp
const double t0 = ms();                      // avant llama_decode
if (llama_decode(x, b) != 0) die("decode");
const double t1 = ms();                      // apres llama_decode
const float * lg = llama_get_logits_ith(x, 0);
const double t2 = ms();                      // apres llama_get_logits_ith
long am = 0;
for (long i = 1; i < LF; ++i) if (lg[i] > lg[am]) am = i;
const double t3 = ms();                      // apres argmax
```

`ms()` = `clock_gettime(CLOCK_MONOTONIC)` sur la même horloge pour les quatre points.

---

## 3 — Les grandeurs publiées

| Nom | Définition | Rôle |
|---|---|---|
| `HOST_DECODE_SUBMISSION_MS` | `t1 − t0` | ce que coûte l'appel `llama_decode` côté hôte |
| `BACKEND_SYNC_WAIT_MS` | `t2 − t1` | ce que coûte la récupération des logits / l'attente de synchronisation |
| `DECODE_PLUS_SYNC_MS` | `t2 − t0` | intervalle où le backend travaille pour ce token |
| `ARGMAX_MS` | `t3 − t2` | coût de la sélection greedy côté hôte |
| `GREEDY_TOKEN_MS` | `t3 − t0` | coût complet d'un token greedy |

Grandeurs dérivées :

```
BACKEND_ACTIVE_INTERVAL_SUM_MS = Σ(DECODE_PLUS_SYNC_MS[i])
BACKEND_EQUIVALENT_TPS         = N / (BACKEND_ACTIVE_INTERVAL_SUM_MS / 1000)
GREEDY_CONTINUOUS_WALL_SECONDS = (t3 du dernier token − t0 du premier)
END_TO_END_GREEDY_TPS          = N / GREEDY_CONTINUOUS_WALL_SECONDS
```

Note de vocabulaire : « soumission hôte » est le nom retenu, mais la mesure montre que
`HOST_DECODE_SUBMISSION_MS` n'est pas une simple soumission asynchrone — voir §4. Le nom est
conservé pour la compatibilité du ledger ; sa sémantique exacte est « coût de l'appel
`llama_decode` tel qu'observé côté hôte ».

---

## 4 — Mesure obtenue (A′, 256 tokens, N = 256)

```
HOST_DECODE_SUBMISSION_MS   p50=0.070  mean=0.210  min=0.062  max=27.976   total=53.784 ms
BACKEND_SYNC_WAIT_MS        p50=64.092 mean=64.015 min=37.007 max=72.345   total=16387.798 ms
DECODE_PLUS_SYNC_MS         p50=64.165 mean=64.225 min=62.479 max=72.418   total=16441.582 ms
ARGMAX_MS                   p50=0.671  mean=0.671  min=0.668  max=0.682    total=171.836 ms

BACKEND_ACTIVE_INTERVAL_SUM_MS=16441.5816
BACKEND_EQUIVALENT_TPS=15.5703
GREEDY_CONTINUOUS_WALL_SECONDS=16.613425
END_TO_END_GREEDY_TPS=15.4092
```

Lecture : **le temps est dans l'attente de synchronisation backend** (64,0 ms), pas dans le retour
d'appel hôte (0,07 ms) ni dans l'argmax (0,67 ms). `t1−t0` est petit parce que `llama_decode`
revient après avoir soumis ; le blocage réel apparaît à `t2−t1`.

Un décrochage unique est visible : `HOST_DECODE_SUBMISSION_MS max = 27,976 ms` (un
ré-ordonnancement de l'hôte sur 256 tokens), qui se retrouve à l'identique dans
`DECODE_PLUS_SYNC_MS max = 72,418 ms`.

---

## 5 — Reproduction exacte

```sh
# compilation (DGX, CUDA 13.2, aarch64)
SRC=/home/triviere/exp-artifacts/engine-003-v3-counters/RUN-001/build-source
LIB=/home/triviere/exp-artifacts/engine-003-v3-counters/RUN-001/build/bin
g++ -O2 -std=c++17 -I$SRC/include -I$SRC/ggml/include aprime.cpp -o aprime \
    -L$LIB -lllama -lggml-cuda -lcrypto -Wl,-rpath,$LIB

# exécution
./aprime /home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf \
         /home/triviere/exp-artifacts/engine-001-qwen3-8-27b-state-native-bench/RUN-001/qualification/prefix805.ids.txt \
         <outdir>
```

Sorties : `<outdir>/aprime-latencies.csv` (une ligne par token, t0..t3 et toutes les grandeurs
dérivées), `aprime-tokens.txt`, `aprime-counters.txt`.

---

## 6 — Ce qui reste ouvert

- `AMORTIZED_GRAPH_MAINTENANCE_COST_PER_TOKEN_MS` n'est **pas** mesuré : il demande de séparer la
  distribution de latence des tokens sans transition et celle des tokens avec transition. A′ ne
  contient qu'un seul token de transition, insuffisant pour une distribution.
- Le CSV de latences est écrit **après** la fenêtre (conforme à `NO_CSV_WRITES`), mais la
  bibliothèque écrit un log debug par token sur stderr : la fenêtre A′ n'est donc pas totalement
  libre d'I/O de diagnostic (voir `docs/ENGINE-004-STATUT-A-B-C-APRIME.md` §6).
