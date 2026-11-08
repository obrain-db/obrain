# Benchmarks — méthode et chiffres de la v0.0.1

Règle : **un chiffre sans commande, matériel et jeu de données n'est pas un
chiffre.** Tout ce que le README affirme sur la performance renvoie ici, et ce
document ne contient que ce que la commande indiquée a produit sur la machine
indiquée. Les benchs ne tournent pas dans le portillon CI (`ci.yml`) : ils
sont lancés à la main (`make bench-v001`) ou par `mesures.yml` (manuel).

## Matériel de référence

| Nom | CPU | RAM | OS | rustc | Disque |
|---|---|---|---|---|---|
| `m4max` | Apple M4 Max (16 cœurs) | 128 Go | macOS 26.5.2 | 1.97.1 | SSD interne (APFS) |

Conditions : charge machine notée au moment de la mesure (`uptime`), profil
`release`, Criterion (`--warm-up-time 1 --measurement-time 5`). Un chiffre
pris à charge > 2 est marqué provisoire.

## Commandes

```bash
make bench-v001            # les quatre benchs ci-dessous, sorties dans target/criterion
cargo bench -p obrain-substrate --bench tiered_scan        # retrieval vectoriel par paliers (10⁶ nœuds)
cargo bench -p obrain-substrate --bench wal_append         # débit d'append WAL
cargo bench -p obrain-engine    --bench query_bench        # requêtes GQL élémentaires
cargo bench -p obrain-engine    --bench phase_a_write_goulot  # chemin d'écriture, commit scellé (D-DUR)
```

## Chiffres — `main` e11bdd2a, machine `m4max`, 2026-11-01 15:22–15:29

Criterion, `--warm-up-time 1 --measurement-time 5`, profil release. **Charge
machine pendant la mesure : 3 à 9** (d'autres compilations tournaient) — ces
chiffres sont donc **provisoires** : bons pour l'ordre de grandeur et la
non-régression relative, pas pour une plaquette. Les chiffres de référence
définitifs seront repris à charge < 2.

| Bench | Mesure | Valeur (médiane) | Charge |
|---|---|---|---|
| `tiered_scan_top10/single_thread/1000000` | retrieval vectoriel par paliers, top-10 sur 10⁶ nœuds, 1 thread | **845 µs** (1,18 Gelem/s) | 8,6 |
| `tiered_scan_top10/rayon_pool/1000000` | idem, pool rayon | **743 µs** (1,35 Gelem/s) | 8,6 |
| `wal_append_unitaire/seul_ecrivain` | un record WAL, un écrivain | 1,22 µs (822 Krec/s) | 5,8 |
| `wal_append_batch/1024` | lot de 1 024 records | 77,4 µs (13,2 Mrec/s) | 5,8 |
| `query_node_lookup_by_property` | `MATCH` par propriété (sans index, 1 000 `Person`) | 379 µs → **52,5 µs** après C1 (voir ci-dessous) | 4,9 / 10–14 |
| `query_1hop_pattern` / `query_2hop_pattern` | motif 1 saut / 2 sauts | 386 µs / 392 µs | 4,9 |
| `query_fan_out_expand_1k` | expansion à 1 000 voisins | 717 µs | 4,9 |
| `query_count_all` | `MATCH (n) RETURN count(n)` | 1,28 µs | 4,9 |
| `query_filter_range` | filtre par plage | 102 µs | 4,9 |
| `query_insert_single_node` | `INSERT` d'un nœud, auto-commit, base en mémoire | 12,6 µs | 4,9 |
| `phase_a_write/sessions_concurrentes_everycommit/4` | 4 sessions concurrentes, `EveryCommit` | 78,4 ms | 3,3 |
| `phase_a_write/writer_unique_group_commit/4` | écrivain unique, group commit | 74,8 ms | 3,3 |
| `phase_a_write/seq_1_thread_group` | séquentiel, 1 thread, `Group` | 78,0 ms | 3,3 |

Ce que le README avançait et que ces chiffres ne confirment PAS tels quels :
« p95 ≤ 1 ms sur 10⁶ nœuds » — la médiane mono-thread est à 845 µs sous
charge, plausible, mais le p95 n'est pas mesuré par ce bench ; « recall
≥ 99 % » — aucun bench ne le mesure. Les deux affirmations sont retirées du
README jusqu'à mesure.

### Coût du commit scellé (D-DUR, PR #22) — mesuré sur `v001/durabilite-commit` a7196d74, même machine, 15:32, charge 8,2

| Bench | `main` e11bdd2a | avec D-DUR | Lecture |
|---|---|---|---|
| `phase_a_write/sessions_concurrentes_everycommit/4` (4 threads × 200 `CREATE` auto-commit sur base persistante `EveryCommit`) | 78,4 ms | **3,56 s** | 800 commits → **≈ 225 commits/s durables** (un `F_FULLFSYNC` par commit, SSD interne Apple). Avant : ≈ 10 000/s, mais rien n'était `fsync` — un commit acquitté pouvait disparaître à la coupure de courant. |
| `phase_a_write/writer_unique_group_commit/4` (`SyncMode::Group`, opt-in) | 74,8 ms | 80,2 ms | inchangé (bruit) : le mode groupé reste le chemin des imports |
| `phase_a_write/seq_1_thread_group` | 78,0 ms | 80,1 ms | inchangé |
| `query_*` (lecture) | 1,28 µs – 717 µs | 1,28 µs – 700 µs | inchangé |
| `query_insert_single_node` (base en mémoire, auto-commit) | 12,6 µs | 13,7 µs | inchangé : `new_in_memory` est en `SyncMode::Never` |

Conclusion : **un commit = un `fsync`** sur une base ouverte par chemin. Pour
écrire vite, grouper les écritures dans une transaction explicite (un seul
`fsync` au `commit()`), ou ouvrir avec `open_substrate_with_mode(SyncMode::Group)`
et appeler `flush_wal()` par lot. C'est le même arbitrage que
`PRAGMA synchronous=FULL` de SQLite.

### Filtre sans index sans hydratation (C1, `perf/filtre-sans-hydratation`) — `m4max`, 2026-11-08 15:08–15:11, base `main` 8f1672e4

`MATCH (n:Person {id: 42}) RETURN n.name` sur 1 000 nœuds, SANS index de
propriété : la planification matérialisait le label puis hydratait CHAQUE
candidat (`get_node_at_epoch` → `Node` complet, labels + toutes les
propriétés) pour lire une seule clef. C1 lit cette clef en lot
(`get_node_property_batch_at_epoch`), sous la même visibilité MVCC, sans
construire de `Node`. Même commande Criterion, passes ALTERNÉES avant/après
(le « avant » rejoué juste avant le « après », même charge) :

| Variante | Médiane | Intervalle | Charge (`uptime`) |
|---|---|---|---|
| avant (filter.rs de `main`) | **508 µs** | 492 – 527 µs | 9,9 → 25 |
| après C1 | **52,5 µs** | 51,3 – 53,9 µs | 14 → 11 |

Gain ≈ 9,7× sur ce bench. `OBRAIN_PHASES=1` (dernières 10 000 requêtes,
médianes ; l'instrumentation gonfle les totaux, seule la ventilation compte) :
planification 845 µs → 56 µs, exécution 10 µs → 8 µs. Le coût était donc la
lecture à la planification, pas l'exécution. Chiffres **provisoires** (charge
10–25), bons pour le rapport avant/après, pas pour une plaquette.

## Ce qui n'est PAS reproductible depuis ce dépôt

- Le tableau LDBC SNB (Obrain vs Kuzu, FalkorDB, Memgraph, Neo4j, ArangoDB)
  vient de l'outil externe `graph-bench`, non versionné ici, sans commit ni
  matériel indiqués. Il est retiré du README tant qu'il n'est pas rejoué avec
  une version épinglée de `graph-bench`.
- `docs/ecosystem/performance.md` portait des tableaux (« ~2M ops/s »,
  Ryzen 9 5900X, LDBC, exemples Python) qu'aucun bench du dépôt ne produit :
  la page est réécrite, elle renvoie ici et ne garde que des règles d'usage.
- `crates/obrain-substrate/benches/REPORT.md` (avril 2026) cite un bench
  `crud_micro` qui n'existe plus : conservé avec un bandeau « historique ».
- `docs/MIGRATION.md` présentait les cibles de la RFC Substrate (ouverture
  ≤ 100 ms, p95 ≤ 1 ms, recall ≥ 99 %) comme des faits : requalifiées en
  cibles, non mesurées.

## Non-régression

`scripts/bench-v001.sh save <nom>` enregistre une ligne de base Criterion ;
`scripts/bench-v001.sh compare <nom>` (ou `make bench-v001-compare BASE=<nom>`)
rejoue les quatre benchs contre elle et sort 1 si Criterion juge qu'une mesure
a régressé au-delà de `BENCH_SEUIL` (10 % par défaut, `--noise-threshold`).
À faire sur une machine calme (charge < 2) : c'est le relevé, pas le portillon.
Mesuré le 01/11 pour fixer les idées : le MÊME arbre, `save` puis `compare`
sur `wal_append` à charge 20 (une autre session compilait), 1 s de mesure —
Criterion a vu « +61 % » sur `wal_append_unitaire` et « −25 % » sur
`wal_append_batch/256`. Rien n'avait changé : c'est la charge qu'il mesurait.
`mesures.yml` lance les mêmes benchs sur un runner partagé, sans seuil, pour
l'ordre de grandeur seulement.

Garde-fous du script (appliqués, pas seulement recommandés) : en `save` et
`compare`, la charge est lue avant et après la passe. À charge ≥ 2 au départ,
le script refuse (sortie 3) sans rien compiler ni mesurer ; à charge ≥ 2 à
l'arrivée, il imprime « ATTENTION charge » et sort 4, pour qu'un relevé
provisoire ne ressemble jamais à une ligne de base propre. La compilation de
chaque bench a lieu avant la boucle mesurée, et une commande cargo en échec
arrête le script (sortie 1). Les lignes de base vivent dans `target/criterion` :
elles ne se comparent qu'entre arbres partageant le même `CARGO_TARGET_DIR`.
