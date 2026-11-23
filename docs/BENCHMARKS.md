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
| `dgx` | NVIDIA GB10 (Grace, aarch64, 20 cœurs) | 121 Go | Ubuntu 24.04.4 LTS, noyau 6.17.0-1029-nvidia | 1.100.0 | NVMe Samsung MZALC4T0HBL1 (ext4) |

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

## Chiffres — `main` a24632c5, machine `m4max`, 2026-11-13 14:27–14:32

Criterion, `--warm-up-time 1 --measurement-time 5`, profil release, une
mesure à la fois via le gouverneur de ressources (`gate.sh exec`), dans un
worktree propre à la tête de `origin/main`. **Charge machine (moyenne 1 min,
`uptime` avant / pendant / après) : 2,31 à 5,40** — au-dessus du seuil de 2,
ces chiffres restent donc **provisoires**. Ils remplacent ceux de e11bdd2a
(charge 3 à 9, eux aussi provisoires) ; les chiffres définitifs seront repris
à charge < 2.

Colonnes « Médiane » et « Intervalle » : valeur centrale et bornes de la
ligne `time: [basse centrale haute]` de Criterion (intervalle de confiance à
95 % du temps par itération), comme pour la référence e11bdd2a. Sorties brutes
non versionnées ; à reproduire avec les commandes ci-dessus.

| Bench | Mesure | Médiane a24632c5 | Intervalle Criterion | e11bdd2a (réf.) | Charge début → fin (max pendant) |
|---|---|---|---|---|---|
| `tiered_scan_top10/single_thread/1000000` | retrieval vectoriel par paliers, top-10 sur 10⁶ nœuds, 1 thread | **836,8 µs** (1,195 Gelem/s) | 831,7 – 842,5 µs | 845 µs | 3,43 → 4,27 (5,40) |
| `tiered_scan_top10/rayon_pool/1000000` | idem, pool rayon | **710,5 µs** (1,408 Gelem/s) | 707,1 – 714,2 µs | 743 µs | 3,43 → 4,27 (5,40) |
| `wal_append_unitaire/seul_ecrivain` | un record WAL, un écrivain | 1,217 µs (822 Krec/s) | 1,179 – 1,258 µs | 1,22 µs | 4,27 → 4,39 |
| `wal_append_batch/1024` | lot de 1 024 records | 78,2 µs (13,1 Mrec/s) | 76,4 – 80,7 µs | 77,4 µs | 4,27 → 4,39 |
| `query_node_lookup_by_property` | `MATCH` par propriété (sans index, 1 000 `Person`) | **47,0 µs** | 46,85 – 47,17 µs | 379 µs (52,5 µs après C1) | 4,39 → 3,28 |
| `query_1hop_pattern` | motif 1 saut | 49,2 µs | 49,09 – 49,31 µs | 386 µs | 4,39 → 3,28 |
| `query_2hop_pattern` | motif 2 sauts | 56,3 µs | 56,17 – 56,37 µs | 392 µs | 4,39 → 3,28 |
| `query_fan_out_expand_1k` | expansion à 1 000 voisins | 717,2 µs | 714,8 – 720,0 µs | 717 µs | 4,39 → 3,28 |
| `query_count_all` | `MATCH (n) RETURN count(n)` | 1,236 µs | 1,232 – 1,241 µs | 1,28 µs | 4,39 → 3,28 |
| `query_filter_range` | filtre par plage | 103,4 µs | 103,2 – 103,7 µs | 102 µs | 4,39 → 3,28 |
| `query_insert_single_node` | `INSERT` d'un nœud, auto-commit, base en mémoire | 15,2 µs | 13,2 – 17,7 µs (6 % de valeurs aberrantes sévères) | 12,6 µs | 4,39 → 3,28 |
| `phase_a_write/sessions_concurrentes_everycommit/4` | 4 sessions concurrentes, `EveryCommit` | **2,134 s** (coût durable, voir ci-dessous) | 2,052 – 2,243 s (10 échantillons) | 78,4 ms (commits non `fsync`és) | 3,28 → 2,31 |
| `phase_a_write/writer_unique_group_commit/4` | écrivain unique, group commit | 79,2 ms | 78,4 – 80,0 ms | 74,8 ms | 3,28 → 2,31 |
| `phase_a_write/seq_1_thread_group` | séquentiel, 1 thread, `Group` | 78,8 ms | 78,0 – 79,4 ms | 78,0 ms | 3,28 → 2,31 |

Écarts notables par rapport à e11bdd2a (255 commits plus tard) :

- `query_1hop_pattern` / `query_2hop_pattern` : ÷ 7,8 et ÷ 7,0, cohérent
  avec le filtre sans hydratation (C1, ci-dessous) qui avait déjà ramené
  `query_node_lookup_by_property` de 379 à 52,5 µs.
- `phase_a_write/sessions_concurrentes_everycommit/4` : **2,13 s contre
  78,4 ms** (× 27). Ce n'est pas une régression : les 78,4 ms de e11bdd2a
  précèdent le commit scellé (D-DUR, PR #22, voir plus bas). À l'époque, un
  commit acquitté n'était pas `fsync`é. Depuis, chaque commit l'est, et sous
  macOS un `fsync` est un `F_FULLFSYNC` (3,56 s mesurés), ramené à
  1,86 – 1,95 s par le `fsync` partagé du 2026-11-08. ~2 s est donc le coût
  durable actuel sur `m4max`, à comparer à 1,86 – 1,95 s et non à 78,4 ms.
  La seconde série ci-dessous (2,058 s) et la mesure en alternance sur la
  même machine (section « Série au repos ») le confirment. Sous Linux
  (ffs-n2), le même bench ne bouge pas à D-DUR.
- Les autres mesures sont dans le bruit (≤ ± 6 %) ou légèrement meilleures
  (`tiered_scan` rayon : − 4 %).

### Seconde série indépendante — `main` a24632c5, `m4max`, 2026-11-13 14:28–14:38

Même commande, même worktree, même gouverneur, benchs lancés l'un après
l'autre. **Toutes les mesures sont provisoires** (charge 1 min > 2 à chaque
mesure). Charge = `uptime` relevé juste avant et juste après chaque bench.

| Bench | Médiane | Intervalle Criterion | e11bdd2a (réf.) | Charge début → fin |
|---|---|---|---|---|
| `tiered_scan_top10/single_thread/1000000` | 841,72 µs | 839,83 – 843,20 µs | 845 µs | 1,66 → 5,93 |
| `tiered_scan_top10/rayon_pool/1000000` | 734,53 µs | 731,54 – 738,06 µs | 743 µs | 1,66 → 5,93 |
| `wal_append_unitaire/seul_ecrivain` | 1,091 µs | 1,081 – 1,103 µs | 1,22 µs | 2,94 → 3,34 |
| `wal_append_unitaire/partage_oappend` | 1,203 µs | 1,168 – 1,242 µs | — | 2,94 → 3,34 |
| `wal_append_unitaire/partage_verrou` | 1,108 µs | 1,099 – 1,117 µs | — | 2,94 → 3,34 |
| `wal_append_batch/1` | 1,180 µs | 1,153 – 1,209 µs | — | 2,94 → 3,34 |
| `wal_append_batch/8` | 1,770 µs | 1,748 – 1,801 µs | — | 2,94 → 3,34 |
| `wal_append_batch/64` | 5,997 µs | 5,950 – 6,050 µs | — | 2,94 → 3,34 |
| `wal_append_batch/256` | 20,40 µs | 20,27 – 20,54 µs | — | 2,94 → 3,34 |
| `wal_append_batch/1024` | 76,54 µs | 76,20 – 76,91 µs | 77,4 µs | 2,94 → 3,34 |
| `query_node_lookup_by_property` | 47,37 µs | 47,28 – 47,46 µs | 52,5 µs (après C1) | 2,81 → 2,27 |
| `query_1hop_pattern` | 49,39 µs | 49,34 – 49,45 µs | 386 µs | 2,81 → 2,27 |
| `query_2hop_pattern` | 56,67 µs | 56,45 – 56,97 µs | 392 µs | 2,81 → 2,27 |
| `query_fan_out_expand_1k` | 724,5 µs | 723,4 – 725,9 µs | 717 µs | 2,81 → 2,27 |
| `query_count_all` | 1,227 µs | 1,224 – 1,231 µs | 1,28 µs | 2,81 → 2,27 |
| `query_filter_range` | 102,8 µs | 102,7 – 103,0 µs | 102 µs | 2,81 → 2,27 |
| `query_insert_single_node` | 12,32 µs | 12,18 – 12,48 µs | 12,6 µs | 2,81 → 2,27 |
| `phase_a_write/sessions_concurrentes_everycommit/4` | 2,058 s | 2,038 – 2,077 s | 78,4 ms (avant D-DUR) ; 1,86 – 1,95 s (fsync partagé) | 2,41 → 2,28 |
| `phase_a_write/writer_unique_group_commit/4` | 71,79 ms | 68,93 – 77,06 ms | 74,8 ms | 2,41 → 2,28 |
| `phase_a_write/seq_1_thread_group` | 67,32 ms | 65,97 – 68,60 ms | 78,0 ms | 2,41 → 2,28 |

Ce qu'il faut savoir sur cette série :

- `wal_append` : un premier passage (14:34:25 – 14:35:24, charge 2,06 → 2,71)
  a été **écarté**, car un second `wal_append` tournait au même moment sur
  un autre créneau du gouverneur (14:34:08 – 14:35:03). Seul le passage
  14:37:43 – 14:38:36, sans aucun autre bench actif, figure ci-dessus.
- `query_bench` (14:35:30 – 14:36:25) et `phase_a_write_goulot`
  (14:36:30 – 14:37:12) ont tourné sans aucun autre bench actif.
- `tiered_scan` (≈ 14:28) a tourné à peu près en même temps que la série
  du dessus. Un chevauchement n'est pas exclu.
- Entre les deux séries, les écarts dépassent le bruit sur
  `writer_unique_group_commit/4` (71,8 contre 79,2 ms) et
  `seq_1_thread_group` (67,3 contre 78,8 ms) : le chemin d'écriture groupé
  varie de 10 à 15 % d'une série à l'autre sous cette charge. Sur toutes
  les autres lignes, les deux séries s'accordent à quelques % près.

### Série au repos — `sessions_concurrentes_everycommit/4`, ffs-n2 (Linux), 2026-11-13 16:18–16:31

Seule série de ce document mesurée **sous le seuil de charge** : ffs-n2, 16
cœurs, Linux, rustc système 1.93, Criterion par défaut, profil release. Chaque
mesure attend qu'aucun job CI ne tourne sur la machine (runner `obrain-ci`) et
que la charge 1 min soit < 1. Elle est déclarée invalide si un job démarre
pendant la mesure (sondage toutes les 5 s : aucun cas). Deux passes, la seconde
dans l'ordre inverse, pour détecter une dérive.

| Commit | Passe 1 | Passe 2 | Charge (avant → après) |
|---|---|---|---|
| `06720e83` (parent de D-DUR) | 640,0 ms [629,7 ; 654,6] | 663,5 ms [638,0 ; 697,4] | 0,81 → 0,92 · 0,71 → 0,91 |
| `a3ae7f3a` (D-DUR) | 630,2 ms [624,5 ; 635,9] | 640,0 ms [628,9 ; 652,8] | 0,92 → 1,09 · 0,94 → 1,18 |
| `a24632c5` | 339,1 ms [326,8 ; 357,5] | 325,5 ms [323,4 ; 327,8] | 0,66 → 0,86 · 0,74 → 0,94 |
| `9802e661` (`main`) | 327,9 ms [322,6 ; 335,8] | 324,3 ms [320,4 ; 328,9] | 0,86 → 1,05 · 0,63 → 0,74 |

Lecture : sous Linux, le `fsync` par commit de D-DUR ne change pas ce bench
(630 contre 640 ms), et `main` est deux fois plus rapide que la référence
(le `fsync` partagé du 2026-11-08 se trouve entre `a3ae7f3a` et `a24632c5`).
Le × 27 du tableau ci-dessus est donc propre à macOS, où un `fsync` est un
`F_FULLFSYNC`.

Même bench sur `m4max` (macOS), 2026-11-13 vers 16:35, commits mesurés en
alternance par le gouverneur. **Provisoire** : charge 25 à 82, mesure
interrompue après 4 points pour libérer la machine. Une mesure alternée reste
comparable entre commits, puisque les trois tournent sous la même charge. Ses
valeurs absolues, elles, ne valent rien.

| Commit | Passe 1 | Passe 2 (ordre inverse) |
|---|---|---|
| `06720e83` (parent de D-DUR) | 116 ms [83 ; 157] | — |
| `a3ae7f3a` (D-DUR) | 4,35 s [4,10 ; 4,59] | — |
| `a24632c5` | 3,78 s [3,56 ; 3,96] | 3,37 s [2,92 ; 3,77] |

Lecture : sur macOS, le saut tombe exactement sur D-DUR (× 37 sous cette
charge), ce qui concorde avec la mesure dédiée plus bas (78,4 ms → 3,56 s,
`F_FULLFSYNC` par commit). Le `fsync` partagé l'atténue ensuite. C'est le coût
de la durabilité, pas une régression. Pour écrire vite sans le payer à chaque
commit : transaction explicite (un seul `fsync`) ou `SyncMode::Group` avec
`flush_wal()` par lot.


### Série alternée — `a24632c5` contre `main` 0e2bd130, `m4max`, 2026-11-22 22:49–22:56

La machine de référence n'est jamais descendue sous 2 depuis le 13/11, et la
série du 13/11 avait elle-même été prise à charge 2,3–5,4. Quand on ne peut pas
tenir le protocole, la comparaison reste possible **à condition d'alterner** :
même gouverneur, même répertoire cible (`slot1`), deux arbres mesurés l'un
après l'autre dans la même fenêtre, donc sous la même charge.

Deux passages, `scripts/bench-v001.sh run` (warm-up 1 s, mesure 5 s, profil
release), charge 1 min relevée avant et après : `main` **3,66 → 5,25**,
`a24632c5` **5,25 → 3,54**. Les deux arbres partagent le répertoire cible : le
second passage recompile les crates locaux avant de mesurer.

| Bench | `a24632c5` (réf.) | `main` 0e2bd130 | Écart |
|---|---|---|---|
| `query_node_lookup_by_property` | 47,32 µs | **3,24 µs** | ÷ 14,6 |
| `query_1hop_pattern` | 49,75 µs | **5,04 µs** | ÷ 9,9 |
| `query_2hop_pattern` | 56,26 µs | **14,49 µs** | ÷ 3,9 |
| `query_count_all` | 1,226 µs | 1,288 µs | + 5 % |
| `query_fan_out_expand_1k` | 687,9 µs | 696,6 µs | dans le bruit |
| `query_filter_range` | 102,84 µs | 102,85 µs | identique |
| `query_insert_single_node` | 12,52 µs | 12,06 µs | identique |
| `wal_append_unitaire/seul_ecrivain` | 1,366 µs | 1,295 µs | − 5 % |
| `wal_append_unitaire/partage_oappend` | 1,383 µs | 1,317 µs | dans le bruit |
| `wal_append_unitaire/partage_verrou` | 1,343 µs | 1,333 µs | identique |
| `wal_append_batch/1` | 1,315 µs | 1,455 µs | + 11 % (seul écart > 10 %) |
| `wal_append_batch/8` | 1,914 µs | 1,997 µs | dans le bruit |
| `wal_append_batch/64` | 6,228 µs | 5,991 µs | dans le bruit |
| `wal_append_batch/256` | 20,06 µs | 20,01 µs | identique |
| `wal_append_batch/1024` | 74,54 µs | 76,00 µs | dans le bruit |
| `phase_a_write/sessions_concurrentes_everycommit/4` | 2,154 s | 2,161 s | identique |
| `phase_a_write/writer_unique_group_commit/4` | 83,25 ms | 82,08 ms | identique |
| `phase_a_write/seq_1_thread_group` | 84,85 ms | 83,14 ms | identique |

Lecture :

- **Les lectures de motif vont 4 à 15 fois plus vite**, et l'écart est si
  grand qu'aucune charge ne peut l'expliquer. Il vient des poussées de prédicat
  et de `LIMIT`, de la graine d'index corrélée et de la jointure par hachage
  (PR #272 à #275), atterries entre les deux arbres.
- **Les écritures n'ont pas bougé** : WAL unitaire et par lots, insert
  unitaire, group commit et `EveryCommit` sont identiques à quelques pour cent.
  C'est le résultat attendu — aucune des PR perf de l'intervalle ne touche le
  chemin d'écriture synchrone.
- Un chiffre pris à charge 12 la même soirée donnait `insert_single_node` à
  40,3 µs (× 3,3) et le WAL de × 2 à × 6. **Aucun de ces écarts n'existe à
  charge comparable** : ils mesuraient la contention de la machine, pas le
  code. C'est la démonstration, sur un cas réel, qu'un chiffre hors protocole
  n'est pas un chiffre.

Ce que cette série ne prouve pas : rien sur le p95 ni sur le rappel (aucun banc
ne les mesure), rien sur les benches absents du tableau, et rien sur une machine
au repos — elle reste **provisoire** au sens du protocole.

<details>
<summary>Ancienne référence — <code>main</code> e11bdd2a, 2026-11-01 15:22–15:29, charge 3 à 9 (provisoire)</summary>

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

</details>

Ce que le README avançait et que ces chiffres ne confirment PAS tels quels :
« p95 ≤ 1 ms sur 10⁶ nœuds » — la médiane mono-thread est à 837 µs sous
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

Mise à jour 2026-11-08 (fsync partagé, `perf/fsync-partage`) : les commits
concurrents d'un écrivain exclusif partagent désormais leur `fsync` (chef de
lot / suiveurs dans `WalWriter::attendre_durable`). Même protocole, mesures en
alternance : `sessions_concurrentes_everycommit/4` avant **3,76 s** [3,71 ; 3,81]
puis **3,71 s** [3,65 ; 3,76] ; après **1,95 s** [1,92 ; 1,98] puis **1,86 s**
[1,81 ; 1,91] (charge 5–8 dans les quatre cas). Le test
`wal_fsync_partage` compte les `fsync` réels : 377 `fsync` pour 800 commits
sur 4 threads (ratio 0,47, mesuré). Un commit seul fait toujours exactement un `fsync` avant
de rendre la main ; la règle « un commit acquitté a été `fsync`é » est
inchangée (test `commits_concurrents_acquittes_survivent_a_un_kill_9`).

Conclusion : **un commit = un `fsync`** (au plus — partagé s'il y a des
commits simultanés) sur une base ouverte par chemin. Pour
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

### Coût de l'unicité composite (PR #99, `fix/contrainte-composite`) — mesuré le 10/11/2026, machine partagée, charge ~3,5

Bench : `cargo bench -p obrain-engine --features cypher --bench contrainte_unique`
(10 000 `INSERT (:T {a, b})` par itération, base en mémoire, échantillon réduit
`sample_size(10)`, warm-up 1 s, mesure 3 s). Temps divisé par 10 000 = µs/insertion.

| Configuration | `main` (avant) | `fix/contrainte-composite` (après) | Écart |
|---|---|---|---|
| sans contrainte | 21,8 µs | 21,9 µs | +0,5 % (bruit) |
| `UNIQUE (a)` simple | 24,6 µs | 24,6 µs | 0 % |
| `UNIQUE (a, b)`, `a` répète (2 valeurs) | 22,0 µs — **2/10 000 insérées** | 743,8 µs — 10 000/10 000 | — |
| idem + `create_property_index("a")` | 22,1 µs — 2/10 000 | 747,1 µs — 10 000/10 000 | — |
| `UNIQUE (b, a)` (colonne sélective d'abord) | — | **25,4 µs** — 10 000/10 000 | — |

Lecture :

- Le correctif **ne régresse pas les chemins communs** : sans contrainte et
  `UNIQUE` simple sont identiques avant/après (0 à +0,5 %, sous le bruit).
- Sur le cas composite avec `a` qui se répète, `main` **rejette** dès la 3ᵉ
  ligne (2/10 000) — c'est le bug. Il n'existe donc pas de « débit avant »
  comparable : le chiffre de `main` (22 µs) mesure un *rejet précoce*, pas une
  insertion.
- Le chemin corrigé coûte **744 µs/insertion** : `find_nodes_by_properties`
  résout le tuple par la **première** propriété contrainte qui a un index.
  `a` étant de faible cardinalité (2 valeurs), l'index sur `a` renvoie ~5 000
  candidats puis filtre sur `b` → O(n). Un index mono-colonne sur `a` n'y
  change rien (747 ≈ 744 µs).
- **L'alternative chiffrée** : déclarer la contrainte colonne-sélective-d'abord
  (`UNIQUE (b, a)`), ou un index de tuple qui résoudrait par `b`, ramène le coût
  à **25,4 µs/insertion** (≈ O(1), ×29). Il n'y a pas d'index composite dans le
  store : c'est la seule voie O(1) aujourd'hui.

Commande exacte pour rejouer :

```bash
cargo bench -p obrain-engine --features cypher --bench contrainte_unique
```


### Série `dgx` — `main` bd9b3b4f, 2026-11-23 15:11–15:15, charge 1,19 → 1,89

Première mesure sur une autre architecture que `m4max` : `dgx` (aarch64, voir le
matériel de référence), clone propre de `origin/main` à `bd9b3b4f`, `make
bench-v001-save BASE=dgx-bd9b3b4`, Criterion `--warm-up-time 1 --measurement-time
5`, profil release, aucune autre mesure en cours. Charge 1 min **1,19 au départ,
1,89 à la fin** : sous le seuil de 2, la série est valide et sert de **ligne de
base** `dgx-bd9b3b4` (dans `target/criterion` du clone `dgx`, non versionné) pour
`make bench-v001-compare BASE=dgx-bd9b3b4`.

La colonne `m4max` est la médiane de la série `a24632c5` du 2026-11-13 ci-dessus,
pour fixer les idées. **Elle ne se compare pas en absolu** : autre architecture,
autre charge (2,3 à 5,4 contre 1,2 à 1,9), et dix jours de commits entre les deux
(`a24632c5` → `bd9b3b4f`). Seule une série alternée sur une même machine départage.

| Bench | Médiane `dgx` | Intervalle Criterion | `m4max` a24632c5 (réf.) |
|---|---|---|---|
| `tiered_scan_top10/single_thread/1000000` | 1,0099 ms | 1,0092 – 1,0105 ms | 836,8 µs |
| `tiered_scan_top10/rayon_pool/1000000` | 1,6388 ms | 1,6019 – 1,6703 ms | 710,5 µs |
| `wal_append_unitaire/seul_ecrivain` | 568,56 ns | 562,89 – 573,83 ns | 1,217 µs |
| `wal_append_unitaire/partage_oappend` | 510,08 ns | 503,46 – 518,30 ns | — |
| `wal_append_unitaire/partage_verrou` | 636,87 ns | 634,53 – 640,59 ns | — |
| `wal_append_batch/1` | 471,01 ns | 467,94 – 477,15 ns | — |
| `wal_append_batch/8` | 1,4444 µs | 1,4318 – 1,4558 µs | — |
| `wal_append_batch/64` | 9,1124 µs | 9,0510 – 9,1837 µs | — |
| `wal_append_batch/256` | 34,093 µs | 34,040 – 34,141 µs | — |
| `wal_append_batch/1024` | 125,52 µs | 125,12 – 126,21 µs | 78,2 µs |
| `query_node_lookup_by_property` | 4,3772 µs | 4,3757 – 4,3789 µs | 47,0 µs |
| `query_1hop_pattern` | 7,0635 µs | 7,0605 – 7,0667 µs | 49,2 µs |
| `query_2hop_pattern` | 21,055 µs | 21,043 – 21,067 µs | 56,3 µs |
| `query_fan_out_expand_1k` | 1,1705 ms | 1,1698 – 1,1714 ms | 717,2 µs |
| `query_count_all` | 2,7604 µs | 2,7600 – 2,7608 µs | 1,236 µs |
| `query_filter_range` | 190,90 µs | 190,68 – 191,17 µs | 103,4 µs |
| `query_insert_single_node` | 19,054 µs | 18,929 – 19,203 µs | 15,2 µs |
| `phase_a_write/sessions_concurrentes_everycommit/4` | 1,7065 s | 1,6808 – 1,7327 s | 2,134 s |
| `phase_a_write/writer_unique_group_commit/4` | 91,350 ms | 89,266 – 93,455 ms | 79,2 ms |
| `phase_a_write/seq_1_thread_group` | 92,054 ms | 84,648 – 99,259 ms | 78,8 ms |

Le même `make bench-v001` joué le même jour sur `m4max` **à charge 25** (une
autre session compilait) donnait 3,22 ms sur `tiered_scan` mono-thread et 17,3 ms
sur `query_fan_out_expand_1k` : **provisoire**, cité ici pour qu'on ne le
prenne jamais pour une régression. La charge mesure la charge.

#### Ouverture d'une base dont le WAL n'a pas été compacté (obrain#264), `dgx`, 2026-11-23 15:39

`cargo run -p obrain-engine --release --features cypher --example profil_open --
200000 1000000` (base synthétique jetable, 200 000 nœuds, 1 000 000 d'arêtes,
82 Mio compactée, 198 Mio avec son WAL), quatre ouvertures successives dans le
même processus, charge 1,3 → 2,2 :

| WAL | open 1 | open 2 | open 3 | open 4 |
|---|---:|---:|---:|---:|
| compacté (`flush` avant la mesure) | 6,7 ms | 5,6 ms | 5,8 ms | 5,9 ms |
| plein, 121 Mo (`SANS_FLUSH=1`) | **546,1 ms** | 73,0 ms | 5,6 ms | 5,6 ms |

Lecture : à WAL compacté, l'ouverture est dans la cible des 5 ms de #264 ; le
« 276 ms à chaud » du ticket est le **premier** open d'un WAL jamais compacté.
Le froid (cache disque vidé) n'a pas été mesuré.

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
- Le banc des **325 requêtes réelles de Project Orchestrator** (section
  « Comparaison obrain / Neo4j » ci-dessous) utilise `spike-migrateur`, un
  harnais du dépôt PO (commit `80025c5`), non versionné ici : il importe une
  copie Neo4j dans obrain, vérifie la parité, puis joue le même plan TSV sur les
  deux moteurs et compare les empreintes. Les scripts exacts, le plan, les
  dumps de résultats et le manifeste sont dans le dépôt de bancs du propriétaire
  (`s3://obrain-bancs/bancs/2026-11-23_bd9b3b4_dgx/`, privé) ; sans eux, cette
  section se lit, elle ne se rejoue pas.
- `docs/MIGRATION.md` présentait les cibles de la RFC Substrate (ouverture
  ≤ 100 ms, p95 ≤ 1 ms, recall ≥ 99 %) comme des faits : requalifiées en
  cibles, non mesurées.

## Bancs sur bases réelles

Une trentaine de cibles de test (`crates/obrain-engine/tests/po_*`, `megalaw_bench`, `t9_*`, `t17*`, `t5_*_po`, `property_index_po_bench`, `search_wikipedia_constellation`, `obrain-substrate/tests/label_scan_cap_bench`, `obrain-cognitive/tests/t17l_hilbert_enricher_on_po`) mesurent ou auditent de **vraies** bases : `po`, `wikipedia`, `megalaw`.

**Elles ne tournent plus par défaut.** Jusqu'au 2026-11-11 elles devinaient `~/.obrain/db/<nom>` à partir de `$HOME` et ne se sautaient que si le dossier manquait. Ce jour-là, un `cargo test -p obrain-engine --features cypher` ordinaire a ouvert en lecture-écriture les bases de travail d'un développeur : `po_vs_neo4j_bench` y a créé puis supprimé des nœuds `_BenchNode` et posé des propriétés `_bench_*` sur des nœuds `File`, et un autre banc a été tué en pleine ouverture. Un test ne doit pas pouvoir faire cela sans qu'on le lui demande.

La règle, portée par le crate interne `obrain-bases-banc` :

- **Sans désignation, rien n'est ouvert.** La cible se saute et l'écrit (`SAUT [<test>] : OBRAIN_BENCH_DB_PO n'est pas définie…` — ou `est définie mais vide` — sur la sortie d'erreur, visible avec `--nocapture`). Aucun chemin n'est jamais déduit de `$HOME`.
- **Une variable par base désigne le répertoire à utiliser** :

  | Base | Variable |
  |---|---|
  | `po` | `OBRAIN_BENCH_DB_PO` |
  | `wikipedia` | `OBRAIN_BENCH_DB_WIKIPEDIA` |
  | `megalaw` | `OBRAIN_BENCH_DB_MEGALAW` |

  Le chemin doit être **absolu** et contenir `substrate.meta`. Sinon la cible échoue avant toute ouverture : une désignation erronée n'est pas un saut, et le moteur créerait une base vide dans un répertoire qui n'en est pas une (désigner par erreur le dossier parent `…/.obrain/db` y sèmerait des `substrate.*`).
- **La base désignée n'est jamais ouverte par le moteur — même en lecture seule.** Elle est copiée dans `TMPDIR` et c'est la copie qu'on ouvre :
  - cibles **lectrices** : une copie par base et par binaire de test (pas une par test, **pas partagée entre binaires** : chaque binaire lancé par `cargo test` prend la sienne), ouverte en lecture seule, supprimée à la sortie du processus. La cible ne reçoit aucun chemin : seulement de quoi ouvrir la copie en lecture seule. Un refus de copie est mémorisé : les tests suivants du binaire échouent avec le même message, sans retenter ;
  - cibles **écrivaines** : une copie privée par appel, supprimée à la fin de la cible — `po_vs_neo4j_bench`, `property_index_po_bench` (crée des index), `t17i_db_open_bench` (mesure l'ouverture en écriture), `t17i_upgrade_then_measure`, `t5_batch_wal_perf_bench_po`, `t5_diagnose_real_bottleneck`, les réparations de `t17j_t4_diagnose`, `t17l_hilbert_enricher_on_po`. `OBRAIN_BENCH_GARDER_COPIE=1` conserve cette copie (`obrain-banc-gardee-*`, chemin imprimé) — c'est ainsi qu'on récupère une base réparée.

### Ce que « lecture seule » garantit aujourd'hui — et pourquoi tout le monde copie

`open_read_only` garantit que **la session refuse les mutations demandées par l'appelant**. Il ne garantit **pas** que rien n'est écrit sur le disque. `SubstrateFile::open_read_only` emprunte le même `open_inner` que l'écriture partagée, et le drapeau `read_only` du store n'est posé qu'après sa construction. Mesuré sur une base laissée par un écrivain interrompu (fenêtre WAL non vide, un graphe nommé), ouverte en lecture seule puis refermée sans une requête : `substrate.nodes`, `substrate.edges`, `substrate.degrees.node.u32`, `substrate.alloc` et les zones du graphe nommé sont réécrits. Sur un répertoire sans `substrate.meta`, `open_read_only` **crée** une base (24 fichiers).

C'est un défaut du moteur, pas encore corrigé. Sa reproduction minimale est dans `crates/obrain-engine/tests/bases_reelles_non_ouvertes.rs`, rouge par construction, derrière le cfg des défauts connus :

```bash
RUSTFLAGS="--cfg defauts_connus" cargo test -p obrain-engine \
  --test bases_reelles_non_ouvertes defaut_moteur -- --nocapture
```

Tant qu'il n'est pas corrigé, la seule protection réelle d'une base est de ne pas l'ouvrir : d'où la copie, pour les lecteurs aussi.

### Ce que coûte la copie — et quand elle est refusée

La copie est **clonée** quand le système de fichiers le permet (`clonefile` sur APFS, `FICLONE` sur Btrfs et XFS avec reflink) : les blocs sont partagés avec l'original, rien de neuf sur le disque jusqu'à ce que le moteur en réécrive. Sinon c'est une copie **pleine mais creuse** : un bloc entièrement nul n'est pas écrit, les trous restent des trous. `OBRAIN_BENCH_COPIE_PLEINE=1` interdit le clone (mesure, essais).

Mesuré sur APFS (macOS, poste de développement chargé, 2026-11-11), bases fabriquées de 1 et 2 millions de nœuds (un `File` avec une propriété et une arête `NEXT` chacun, `TMPDIR` sur le même volume) :

| Base | Taille logique | Taille physique | Clone | Copie pleine |
|---|---|---|---|---|
| 1 M nœuds | 4 242 Mio | 4 030 Mio | 0,09 s, 0 Mio neufs | 3,62 s, 4 043 Mio |
| 2 M nœuds | 9 440 Mio | 8 025 Mio | 0,05 s, 0 Mio neufs | 13,65 s, 8 037 Mio |

À 5 M de nœuds la base ferait ≈ 20 Gio physiques (la taille est linéaire : 4,0 Gio par million) : **non mesuré**, parce que la fabriquer seule dépasse le plafond de 20 Go de disque cumulé fixé pour ces mesures. Le clone resterait à quelques centièmes de seconde ; une copie pleine demanderait ≈ 20 Gio de disque et plusieurs dizaines de secondes.

Le clone n'existe **qu'à l'intérieur d'un même volume**. Hors de là, ou sur un système de fichiers sans copy-on-write, la copie est pleine, et le helper la **refuse** au lieu d'emplir le disque :

- **plafond** : une copie pleine de plus de 64 Gio est refusée (`OBRAIN_BENCH_COPIE_MAX_GO`) ;
- **marge** : une copie pleine qui laisserait moins de 10 Gio de disque libre est refusée (`OBRAIN_BENCH_MARGE_GO`) ; le clone est contrôlé contre la même marge.

Les deux refus sont nommés et cités avec leur variable. Pour mesurer avant de se lancer :

```bash
cargo run --release -p obrain-bases-banc --example cout_copie -- /chemin/absolu/vers/la/base
```

**Une copie juste ou pas de copie.** La copie est refusée, nommément, si :
- un processus **tient** la base : le moteur pose un verrou `fcntl` sur `substrate.dirlock` pendant toute la vie d'une ouverture, que le helper interroge avec `F_OFD_GETLK` (Linux, macOS) sur un descripteur ouvert en lecture seule : rien n'est écrit, et le processus **appelant** est vu comme un autre (`F_GETLK` y serait aveugle). Ce descripteur n'est jamais fermé : fermer un descripteur de ce fichier lâcherait le verrou que le moteur de ce processus tiendrait. Un lecteur compte aussi, puisqu'une ouverture en lecture seule écrit. Une base sans `substrate.dirlock` (jamais ouverte par un moteur qui le pose) : aucun détenteur ne peut être cherché, le helper le dit, et seule la comparaison suivante protège — comme hors Unix ;
- la base a **changé** pendant la copie : liste des fichiers, tailles et dates comparées avant et après ;
- elle contient un **lien symbolique** ou un fichier spécial : refusé, jamais suivi (un lien qui boucle ferait déborder la pile, un lien cassé paniquer).

Une base en `0444` (instantané en lecture seule) donne une copie rendue inscriptible. Désigner un instantané reste le plus sûr.

**Résidus.** Les copies s'appellent `obrain-banc-<pid>-<base>-*` dans `TMPDIR`. Elles sont supprimées à la fin de la cible ou à la sortie normale du processus. Un processus tué (Ctrl-C, `kill -9`) laisse la sienne ; son propriétaire tenait un verrou exclusif sur `<copie>/.proprietaire`, que le noyau a rendu à sa mort : le prochain banc lancé supprime les copies dont il peut prendre ce verrou (Unix). La preuve de vie est un verrou, pas un PID : un PID peut être recyclé ou appartenir à un autre espace de noms. Sous Windows, supprimer les résidus à la main.

**En CI.** Le job « Bancs sur bases fabriquées » de `ci.yml` fabrique de petites bases jetables (`cargo run -p obrain-substrate --example fabrique_bases_banc`, `cargo run -p obrain-engine --features cdc --example fabrique_base_cdc`, `fabrique_base.rs`), les désigne, et joue `byte_identical_corpus`, `mesures_corpus`, `cout_d_une_consolidation_sur_store_reel`, `banc_surcout` et `verify_bases` ; il vérifie ensuite que les bases désignées n'ont pas bougé, et que les reproductions des défauts connus du moteur (`defaut_moteur_*`, sous `--cfg defauts_connus`) sont **toujours rouges** — le jour où elles passent, c'est que le moteur a changé et que le cfg doit être retiré. La sonde de consolidation échoue, nommément, sur une fenêtre CDC vide au lieu de sortir verte sans rien mesurer. Restent manuels : les bancs sur les trois bases réelles (`OBRAIN_BENCH_DB_*`), par construction.

### Lancer un banc volontairement

```bash
OBRAIN_BENCH_DB_PO=/chemin/absolu/vers/po \
  cargo test -p obrain-engine --release --features cypher \
  --test po_vs_neo4j_bench -- bench_po_vs_neo4j --exact --nocapture

# Les trois corpus
OBRAIN_BENCH_DB_PO=/chemin/po OBRAIN_BENCH_DB_WIKIPEDIA=/chemin/wikipedia OBRAIN_BENCH_DB_MEGALAW=/chemin/megalaw \
  cargo test -p obrain-engine --release --features cypher --test t17i_ram_bench -- --nocapture

# La batterie de non-régression (exige OBRAIN_BENCH_DB_PO)
OBRAIN_BENCH_DB_PO=/chemin/po scripts/bench-regression.sh
```

Les cibles marquées `#[ignore]` (audits ponctuels, bancs longs) demandent en plus `-- --ignored`.

Deux tests verrouillent la règle et tournent, eux, à chaque `cargo test` : `crates/obrain-engine/tests/bases_reelles_non_ouvertes.rs` fabrique de vraies bases — dont une sale — dans un faux `HOME` temporaire, joue un banc lecteur et un banc écrivain en sous-processus et exige une empreinte identique à l'octet (liste des fichiers, tailles, dates des fichiers et des répertoires, contenus) ; `crates/obrain-bases-banc/tests/garde.rs` refuse tout fichier du dépôt (sources des crates — le helper compris —, tests, bancs, exemples, paquets, scripts avec ou sans extension, manifestes et configurations, Makefile, workflows) qui lirait le répertoire personnel (`"HOME"`, `os.environ["HOME"]`, `process.env.HOME`, `:HOME`…), construirait un chemin `.obrain`, énumérerait l'environnement, ou nommerait une variable `OBRAIN_BENCH_DB_*` hors du helper (dont le nom n'est d'ailleurs pas exporté) — hors exceptions nommées, à la ligne et au nombre d'occurrences près. Les **blocs de code des commentaires de documentation** (`///`, `//!`) sont examinés comme du code, puisque `cargo test` exécute les doctests ; seule la prose et les blocs d'un autre langage sont sautés. Le préfixe de commentaire est propre à chaque famille (`::` n'en est un que dans un `.bat`). Un répertoire ou un fichier illisible fait échouer la garde ; chaque contournement connu — dont les deux trouvés par la relecture de #113 — est planté dans un faux dépôt et doit être refusé. La garde reste **textuelle** : un nom de variable reconstitué à l'exécution lui échappe ; la barrière de fond est que le helper ne rend jamais que des copies.

### Ce qui ouvre encore directement le chemin qu'on lui donne

**Règle générale : tout exemple (`cargo run --example …`) et tout outil qui prend un chemin en argument l'ouvre tel quel — en écriture sauf mention contraire, et même « en lecture seule » il peut y écrire (défaut ci-dessus).** Rien n'y est deviné, mais rien n'y est copié non plus : leur passer une copie, jamais une base vivante.

| Cible | Désignation | Ce qu'elle fait au chemin reçu |
|---|---|---|
| `obrain-engine/tests/fabrique_base.rs` (`#[ignore]`) | `OBRAIN_BANC_BASE` | **crée** une base (refuse si le chemin existe) — c'est son objet |
| maintenance : `add_name_index`, `fix_file_names`, `mig_slots512`, `backfill_tms`, `slots512_qc`, `conv_micros`, `cree_store_melange_j4` (engine) ; `compact_blob_columns`, `strip_skipped_props`, `rebuild_tiers`, `vectorize_list_props`, `checkpoint_cli` (substrate) | argument | modifient la base — c'est leur objet |
| `banc_ns_j2`, `sim_conversations`, `perf_churn`, `perf_point_lookup`, `bench_texte_gql`, `bench_intention_contient` (engine) ; `inspect`, `replay_bench`, `bench_trigrammes`, `cognitive_quality` (substrate) ; `persist` (indexer, crée la base de sortie) ; `examples/rust/bench_load.rs` | argument | ouverture en écriture (`bench_texte_gql` et `bench_intention_contient` : en lecture seule si `OBRAIN_LECTURE_SEULE=1`) |
| `banc_get_egalite`, `banc_open_ro_j4`, `sonde_j4`, `sonde_edges_j4`, `bench_agregat_groupe` (engine) ; `position_scan` (cognitive) | argument | `open_read_only` — peut donc y écrire |
| `audit_props`, `probe_prop_shape` (substrate) | argument | lecture de fichiers, sans passer par le moteur |
| `bench_corpus`, `bench_index`, `bench_queries`, `queries` (indexer) | argument | lisent un dépôt de sources, pas une base |
| `bench_checkpoint_elu`, `bench_contention_n` (substrate) | `CHECKPOINT_DIR` (défaut `/tmp/obrain-bench-checkpoint.obrain`) | créent et écrivent leur propre base |

Passent désormais par une copie privée (`obrain_bases_banc::copie_de`, chemin absolu contenant `substrate.meta`), au lieu d'ouvrir le chemin reçu :

| Cible | Désignation |
|---|---|
| `obrain-engine/benches/chemin_nanoseconde.rs` | `NANO_STORE` |
| `obrain/benches/zero_copy.rs` | `OBRAIN_ZC_STORE` |
| `obrain-engine/tests/cout_consolidation.rs` | `OBRAIN_SONDE_STORE` |
| `obrain-engine/tests/banc_surcout.rs` | `OBRAIN_BANC_BASE` |
| `obrain-engine/examples/banc_plan_froid` | argument ou `OBRAIN_BANC_BASE` |
| `obrain-substrate/tests/simplex_passes_materialisation.rs` | `CORPUS_A`, `CORPUS_B`, `CORPUS_DB` |
| `obrain-substrate/tests/tenant_colonne.rs::migration_store_reel_par_copie` | `OBRAIN_TEST_STORE_REEL_DIR` |
| `obrain-engine/examples/verify_bases` | arguments |

Les cibles `bench-baseline*`, `bench-substrate*`, `bench-compare`, `bench-smoke` et `check-substrate` du `Makefile` sont **retirées** : elles lançaient un harnais `bench/substrate` qui n'existe plus, sur des bases désignées par `PO_DB` / `MEGALAW_DB` hors du helper.

## Comparaison obrain / Neo4j sur les 325 requêtes réelles de PO — `dgx`, 2026-11-23

**Ce que cette section est** : les 325 requêtes de lecture que Project
Orchestrator envoie à sa base (plan `plan_reel.tsv`, paramètres réels), jouées
sur le **même jeu de données** par les deux moteurs, sur la même machine, l'un
après l'autre (jamais en même temps). **Ce qu'elle n'est pas** : un banc
reproductible depuis ce dépôt (voir ci-dessus), ni une mesure à froid.

### Jeu de données : un clone, pas la copie figée

La copie obrain figée `po-frozen-20261117` (3 901 829 nœuds, 6 013 429 relations)
n'a plus de source Neo4j : sa référence `resultats_neo4j.tsv` du 17/11 décrit un
**autre** instantané (4 105 288 nœuds). Comparée à lui, elle donnait 230 écarts
sur 325 — presque tous des écarts de **données**, pas de moteur. Un chiffre
d'exactitude n'a de sens que sur un jeu identique des deux côtés.

Le jeu retenu est donc un **clone Neo4j** de la base vivante de PO, pris le
23/11 puis arrêté (Neo4j 5.26.20 community dans Docker 29.2.1, tas 8 Go, cache de
pages 16 Go, sans APOC) : **4 327 350 nœuds, 64 labels ; 6 549 502 relations,
81 types**. La base obrain est **importée de ce clone** :

| Import (`spike-migrateur import`, 8 lecteurs bolt, lots de 20 000) | |
|---|---:|
| A — identifiants et labels des nœuds | 3,1 s (1 418 764 nœuds/s) |
| B — relations et leurs propriétés (104,0 Mo) | 8,5 s (766 604 relations/s) |
| C — `bulk_import_refs` (nœuds + relations) | 0,59 s |
| D — 32 531 119 propriétés de nœuds, 6 127 Mo utiles | 42,1 s (102 827 nœuds/s, 145,6 Mo/s) |
| E — 5 296 457 propriétés sur 2 122 473 relations | 5,1 s |
| F — checkpoint et fermeture | 15,7 s |
| **Total** | **75,3 s**, RSS max **20,69 Go**, base **31,0 Go alloués** (52,6 Go de longueur logique) |

Vérification (`spike-migrateur verif`, 46,4 s, RSS 4,95 Go) : comptes par label
et par type **sans écart** (64 et 81 noms, mêmes sommes), **36 709 554**
propriétés de nœuds et **5 296 457** de relations des deux côtés ; sur un
échantillon de 1 247 nœuds (13 099 propriétés) et 1 473 relations
(1 185 propriétés), **0 écart** sur tous les types sauf `LIST<FLOAT>` : 79
embeddings sur 204 contrôlés diffèrent **au-delà de la précision f32**
(`0.005389043` lu `0.005389043`, non exact en f32), c'est la représentation,
pas la donnée. Aucune perte de conversion (fuseau, sous-microseconde, type sans
équivalent : 0).

### Méthode

Plan TSV de 325 requêtes ; chaque requête est jouée 3 fois à blanc puis **15
fois mesurées** (budget 30 s), médiane. Les résultats de chaque exécution sont
canonisés (colonnes triées par nom, nœud rendu par sa propriété `id`/`path`/…)
et hachés (FNV-1a 64) **hors chronomètre** ; les deux moteurs passent par les
mêmes `Value` d'obrain. L'empreinte du harnais dépend de l'**ordre des lignes**
: deux moteurs qui rendent les mêmes lignes dans un ordre différent ont des
empreintes différentes. L'exactitude est donc relue sur les dumps, en
**ensembles** (ordre ignoré, flottants arrondis à 9 chiffres, listes triées).

Neo4j : client bolt (`neo4rs` 0.8, corrigé pour les entiers −16..−1) en
processus séparé sur la même machine. obrain : **en processus**, `ObrainDB::
open_read_only`. Les deux ont tourné **l'un après l'autre**, Neo4j arrêté
pendant la mesure obrain.

### Exactitude — moteur `bd9b3b4f`, index natifs présents

Les index de Neo4j (188 RANGE, 8 VECTOR, 58 contraintes) sont recréés côté
obrain : 8 index vectoriels (768 dimensions, cosinus) et **60 des 71**
propriétés de nœud indexées (11 absentes après réouverture, non instruites) ;
`spike-migrateur index-natif`, 26,2 s, RSS 62,1 Go. Sans ces index, la première
passe donnait les **mêmes équivalences** et une somme des médianes de 332,7 s
contre 329,3 s : sur ce plan, les index de propriété pèsent 1 %, la différence
avec Neo4j est ailleurs (voir « Temps »).

Sur **307 requêtes qui rendent un résultat des deux côtés** :

| Verdict (ensembles normalisés) | Requêtes |
|---|---:|
| identiques | **173** |
| identiques **à une colonne près** : la clé de tri (`e.seq`, `d.decided_at`…) apparaît comme colonne `e_seq` côté obrain | **79** |
| 0 ligne des deux côtés (colonnes non comparables sur un résultat vide) | 13 |
| mêmes lignes, **valeurs différentes** | 28 |
| **nombre de lignes différent** | 8 |
| valeurs différentes et colonne fuitée | 6 |

Soit **265 équivalentes sur 307** si l'on tolère la colonne fuitée, 173 au
sens strict. Les 28 + 6 écarts de valeurs mêlent le bénin (format de date
`+00:00` contre `Z` : 4 ; départage arbitraire sur un score égal : au moins 3) et
le réel, non classé exhaustivement : `NULL` là où Neo4j rend une valeur
(`ana_14`, `ana_10`, `plan_06`, `schema_06`), `COALESCE(r2.source, 'manual')` qui
rend `'manual'` (`chat_26`), une liste de maps rendue `[{}]` (`ws_19`), des
comptes qui divergent (`dec_10` 253/251, `ws_20` 2 131/911). Les 8 nombres de
lignes différents : `code_21` et `code_37` rendent 0 ligne (Neo4j 20 et 67),
`sampler_01` 500 (Neo4j 7 615), `plan_12` 8 511 (8 751), `ana_22` et `chat_25`
0 (1), `code_48` 443 (441), `plan_05` 11 (12).

**Erreurs** : obrain **18**, Neo4j **3**. Les 3 de Neo4j sont des fonctions
APOC absentes du conteneur (`apoc.coll.toSet`, `apoc.coll.sort`,
`apoc.meta.schema`) : hors sujet, et deux d'entre elles échouent aussi côté
obrain pour la même raison. Les 15 propres à obrain, par famille :

| Famille | Requêtes | Message |
|---|---|---|
| écriture refusée en **lecture seule** sur des procédures de lecture | `note_31/32/33/35`, `dec_04`, `chat_07` (`db.index.vector.queryNodes`), `schema_01/02` (`db.labels`, `db.relationshipTypes`) | `Cannot write in read-only transaction` |
| `PatternComprehension` non réécrite | `chat_23` (`[…][0]` après un `WITH` agrégeant), `persona_10` (dans un `WHERE`) | `reached the planner without being rewritten` |
| sous-motif corrélé par un nœud qui n'est **pas le premier** | `nbh_01`, `nbh_02` (`[(op:Project)-[:HAS_PLAN]->(m) \| op.id]`) | `sous-requête de motif corrélée par 'm'… non prise en charge` |
| identifiant interne **partagé nœud/arête** dans une liste | `ana_09` (`nodes(path)`), `ws_14` (`collect(DISTINCT t1) + collect(DISTINCT t2)`) | `désigne à la fois un nœud et une arête` |
| produit cartésien refusé | `code_44` | `scan imbriqué (produit cartésien) refusé` |
| `WITH *, expr AS alias` | `ana_11` | `Expected end of query` à la virgule après `WITH *` |

Chaque famille a son issue, avec une reproduction **en mémoire** quand elle
existe (six formes sur sept ; `code_44` ne se reproduit pas sur un petit graphe,
la garde étant une estimation de cardinalité) : lecture seule #354 ;
`PatternComprehension` #357 ; corrélé par un nœud non premier #355 ; identifiant
partagé **#229 rouverte** ; produit cartésien **#228 rouverte** ; `WITH *, …`
#356. #228 et #229 avaient été fermées par #231 sur la forme de `chat_11` et
`topo_04` : la forme de PO n'est pas celle que leurs tests figent. La colonne
de tri fuitée des 79 + 6 requêtes ci-dessus : #353.

### Temps — moteur `bd9b3b4f`, index natifs présents, **avant** #345

Somme des médianes sur les 307 requêtes : **329,3 s côté obrain, 31,1 s côté
Neo4j**. Rapport médian géométrique : **1,22** ; obrain est plus rapide sur 139
requêtes, plus lent sur 168. La somme est portée par quelques traversées :

| Requête | obrain | Neo4j | rapport |
|---|---:|---:|---:|
| `code_45_find_bridge_subgraph_noeuds_imports_1saut` | 21 410 ms | 2,9 ms | × 7 275 |
| `code_40_find_impacted_files_depth3` | 18 192 ms | 6,5 ms | × 2 794 |
| `code_24_get_function_callers_by_name_depth2` | 6 206 ms | 3,2 ms | × 1 953 |
| `code_26_get_callers_with_confidence` | 4 510 ms | 2,8 ms | × 1 636 |
| `code_33_get_function_caller_count` | 4 975 ms | 3,3 ms | × 1 521 |
| `topo_04_check_max_distance` (`shortestPath` ×50 paires) | 80 068 ms | 255 ms | × 314 |
| `ws_20_compute_coupling_matrix_shared_notes` | 36 914 ms | 2 473 ms | × 15 |
| `note_23_get_propagated_notes_file_api` | 19 941 ms | 12 897 ms | × 1,5 |

Et à l'inverse : `code_11_get_class_hierarchy_parents` 0,05 ms contre 1 526 ms,
`code_46_find_bridge_subgraph_aretes_imports` 0,28 ms contre 591 ms, les
lectures par clé (`proj_09`, `proj_15`, `code_03`) 0,01 à 0,02 ms contre 3 à 9 ms.

### Temps — moteur `4a37c0be` (`main` après #345), même base, mêmes index, même plan

Rejeu des 325 requêtes avec le seul changement de moteur (`bd9b3b4f` → `4a37c0be`,
qui ajoute #345 : `EXISTS` corrélé quand son point de départ et une extrémité sont
déjà liés) ; `dgx`, charge 1,0 → 2,3 au départ de la mesure (1,45 à son démarrage),
même procédure (3 échauffements, 15 mesures, budget 30 s).

| | `bd9b3b4f` | `4a37c0be` |
|---|---:|---:|
| somme des médianes (307 requêtes ok des deux côtés) | 329,3 s | **281,5 s** |
| rapport médian géométrique obrain / Neo4j | 1,22 | **1,14** |
| requêtes où obrain est plus rapide | 139 | 140 |
| durée du banc | 34 min 01 s | 30 min 35 s |
| RSS max d'obrain | 86,1 Go | **43,8 Go** |
| requêtes dont l'**empreinte** a changé entre les deux passes | | **0** |
| verdicts normalisés (équivalentes / valeurs / lignes / colonnes) | 173 / 79 / 28 / 13 / 8 / 6 | identiques |

Les résultats n'ont pas bougé d'un octet : aucune des requêtes qui rendent un
résultat ne change d'empreinte. Seules **quatre** requêtes changent de plus d'un
facteur 2 :

| Requête | avant | après | rapport |
|---|---:|---:|---:|
| `code_45_find_bridge_subgraph_noeuds_imports_1saut` | 21 410 ms | **13,6 ms** | × 1 570 |
| `code_38_find_dependent_files_depth2` | 2 520 ms | **5,0 ms** | × 503 |
| `code_22_find_symbol_references_file_imports` | 2 764 ms | **84,9 ms** | × 32,6 |
| `code_48_find_bridge_subgraph_noeuds_defaut_api_3sauts` | 19 643 ms | **4 854 ms** | × 4,0 |

`code_38` et `code_22` n'étaient pas visées : leur gain n'est pas expliqué, je ne
l'attribue pas.

**Ce que #345 ne corrige pas — et ce qui précède (version précédente de ce
paragraphe) disait à tort le contraire.** `code_40` (18 192 → 16 434 ms),
`code_24`, `code_26`, `code_33`, `code_25`, `code_41`, `code_20`, `code_27` : de
4,5 à 16,4 s avant, de 4,7 à 16,4 s après, **inchangées**. Seule la forme
`EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) }`, sans nœud intermédiaire, est réparée.

Reproduction en mémoire (`examples/mesure_exists_formes.rs`, 2 000 → 8 000 → 32 000
fichiers, résultat constant 5, médiane de 3) :

| Forme | 2 000 | 8 000 | 32 000 | Suit le graphe ? |
|---|---:|---:|---:|---|
| A `EXISTS { n<-CONTAINS*1..2-p }` (réparée) | 0,09 | 0,08 | 0,08 ms | non |
| B `EXISTS { n<-CONTAINS-(:File)<-CONTAINS-p }` (intermédiaire **étiqueté**) | 4,98 | 20,2 | 90,1 ms | **oui** |
| C `OPTIONAL MATCH … WHERE EXISTS { n<-CONTAINS*1..2-p }` | 2,67 | 10,5 | 49,0 ms | oui, mais le témoin E aussi |
| D `OPTIONAL MATCH … WHERE EXISTS { chaîne étiquetée }` (`code_33`) | 7,18 | 29,9 | 138,8 ms | **oui** |
| E témoin : D **sans** `EXISTS` | 2,51 | 10,2 | 47,3 ms | oui (voir ci-dessous) |

Le témoin E croît lui aussi : ce graphe en mémoire n'a pas d'index sur
`Function.name`, donc le `MATCH` de départ balaie les fonctions, et ce coût est
**le même** avec ou sans `EXISTS`. Il se **soustrait** : l'`EXISTS` de la forme C
coûte 1,7 ms de plus que le témoin (aucun surcoût), celui de la forme D en coûte
**91 ms** de plus (× 4 par × 4 de taille). Donc : une chaîne passant par un nœud
intermédiaire étiqueté n'est pas réécrite et reste en coût du graphe ; un `EXISTS`
sous `OPTIONAL MATCH` n'est pas en cause.

**Non établi** : pourquoi la forme B n'est pas réécrite (hypothèse non vérifiée :
le traducteur représente `(:File)` comme un `NodeScan` étiqueté **chaîné**, et la
réécriture le refuse volontairement) ; et la cause de `code_40`, dont les deux
`EXISTS` sont directs (sans intermédiaire) mais placés sous `OPTIONAL MATCH … WHERE
… OR EXISTS`, un chemin que cette reproduction ne couvre pas.

Sur la base PO (4,3 M de nœuds, 130 fois le plus grand graphe ci-dessus), un
surcoût de 91 ms à 32 000 fichiers donne un ordre de grandeur de 10 s, compatible
avec les 4,5 à 6,4 s observés : une cohérence, pas une preuve.

**Ressources** : obrain, processus unique, **RSS max 86,1 Go** avec index
(81,1 Go sans), ~100 % d'un cœur, 34 min 01 s pour les 325 requêtes ; Neo4j
**16,2 Gio** et jusqu'à 448 % CPU dans le conteneur, 6 min 21 s. Le pic de RAM
d'obrain n'est **pas attribué** à une requête (échantillon RSS toutes les 2 s,
pas par requête) : c'est le premier chiffre à instruire.

### Ouverture et `obrain-indexer`, même machine, même journée

`mesure_ouverture` sur `po-frozen-20261117` (3,9 M nœuds, 29 Go), trois
ouvertures dans le même processus : **lecture seule** 27,8 / 1,45 / 1,41 ms,
première requête de comptage 337 / 27 / 27 ms, RSS 786 Mo (charge 1,9) ;
**écriture** 76,7 / 53,7 / 53,3 ms, RSS 774 Mo — **provisoire**, charge 15.

`obrain-indexer` (`cargo run --release -p obrain-indexer --example bench_index`),
trois essais, charge 1,3 à 1,5 : `obrain-db` à `bd9b3b4f` (1 326 fichiers
parsés sur 1 362 découverts, 33 993 nœuds, 242 944 arêtes) **1,54 / 1,42 / 1,46 s**,
RSS 484 Mo ; `rustorch-r1` à `32a30ed` (984 fichiers, 17 570 nœuds, 60 612
arêtes) **0,54 / 0,47 / 0,48 s**, RSS 261 à 269 Mo. Persistance dans une base
obrain (`example persist`) : 4,47 s au total dont 1,57 s d'écriture, RSS 611 Mo,
187 Mo sur disque.

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

### Coût de l'unicité sur tous les chemins d'écriture (PR « unicité sur tous les chemins ») — mesuré le 11/11/2026

Banc : `UNICITE_N=<N> cargo bench -p obrain-engine --features cypher --bench unicite_ecritures`
(base substrate sur disque semée par l'API directe, WAL sans fsync — ce coût est
le même avec ou sans contrainte —, 2 000 instructions par cellule, µs par
instruction, parse + plan compris, machine partagée très chargée : comparer
chaque ligne à `sans_contrainte` DU MÊME bloc, pas d'un bloc à l'autre).
Colonnes : `a` et `b` uniques, `g` à deux valeurs.

| N | configuration | INSERT | SET col. contrainte | SET col. libre | MERGE trouve | MERGE crée |
|---|---|---|---|---|---|---|
| 100 k | sans contrainte | 25,3 | 16,6 | 15,6 | 15,8 | 25,4 |
| 100 k | `UNIQUE (a)` | 27,9 | 16,8 | 14,9 | 15,8 | 26,7 |
| 100 k | `UNIQUE (b, g)` | 27,5 | 18,3 | 15,6 | 17,7 | 28,3 |
| 1 M | sans contrainte | 45,3 | 34,6 | 34,7 | 49,7 | 101,0 |
| 1 M | `UNIQUE (a)` | 67,4 | 38,1 | 41,1 | 39,3 | 84,2 |
| 1 M | `UNIQUE (b, g)` | 58,3 | 35,1 | 34,6 | 40,6 | 67,3 |
| 5 M | sans contrainte | 37,5 | 24,4 | 21,6 | 21,8 | 36,5 |
| 5 M | `UNIQUE (a)` | 46,9 | 24,5 | 21,1 | 23,1 | 37,7 |
| 5 M | `UNIQUE (b, g)` | 41,9 | 20,3 | 17,8 | 19,6 | 30,8 |

Lecture :

- **Le coût du contrôle ne dépend pas de N** quand la colonne de tête du tuple
  est sélective (`UNIQUE (a)`, `UNIQUE (b, g)`) : de 100 000 à 5 000 000 nœuds il
  reste de l'ordre de quelques µs par écriture, dans le bruit de la machine. Le
  `SET` d'une colonne contrainte ne coûte pas plus qu'un `SET` libre : il lit
  les seules colonnes contraintes du nœud, jamais le nœud entier.
- **Avant** (tête de #99, mêmes bancs) : `SET` et `MERGE … ON MATCH SET` n'étaient
  pas contrôlés du tout (leur coût plat ne mesure aucun contrôle).
- **Cas qui croît avec N — non corrigé ici** : `UNIQUE (g, b)` (colonne à deux
  valeurs EN TÊTE). `find_nodes_by_properties` résout par la première colonne
  indexée, donc relit la moitié du label : 27 ms/op à 100 k, 430 ms à 1 M,
  1,4 s à 5 M (même ordre qu'avant cette PR pour l'INSERT : 43 ms, 548 ms,
  2,4 s). Le correctif est la PR #101 (résolution par l'ensemble indexé le plus
  petit) ; en attendant, déclarer la colonne sélective en premier. **Mesuré avec
  #101 fusionnée dans un arbre jetable, 1 M nœuds : `UNIQUE (g, b)` = 39,6 µs
  INSERT, 25,2 SET, 16,4 MERGE trouve, 34,1 MERGE crée** (contre 433 000 µs sans
  #101) : le coût ne dépend plus de N, et les six tests de
  `unicite_valeurs_egales.rs` passent au vert.
- **`CREATE CONSTRAINT` sur données existantes** (passe de vérification, une fois) :
  210 ms (`UNIQUE (a)`) et 350 ms (`UNIQUE (b, g)`) à 1 M nœuds ; 755 ms et
  1,4 s à 5 M. C'est la même passe que `ObrainDB::verifier_contraintes` :
  linéaire, une table de hachage de tuples, aucune recherche d'index.

#### Compléments (relecture) : colonne flottante, écrivains parallèles — 11/11/2026

Mêmes conditions (base disque, µs par instruction), branche contenant la résolution par l'ensemble indexé le plus petit :

| N | config | INSERT | SET col. contrainte | MERGE trouve | MERGE crée |
|---|---|---|---|---|---|
| 100 k | `UNIQUE (g, b)` | 34,8 | 16,2 | 13,0 | 23,3 |
| 100 k | **`UNIQUE (f)`, `f` flottant non entier** | **26 499** | **26 000** | **25 980** | **24 789** |
| 1 M | `UNIQUE (g, b)` | 39,1 | 18,9 | 14,9 | 29,1 |
| 1 M | **`UNIQUE (f)`, `f` flottant non entier** | **236 521** | **227 396** | **241 680** | **232 096** |

- **Une contrainte sur une colonne flottante non entière coûte O(taille du label) par écriture** (26 ms à 100 k, 237 ms à 1 M) : l'index d'égalité ne projette pas ces valeurs (seuls Bool, Int64, String, flottants entiers et instants-chaînes le sont), la recherche retombe sur un balayage. C'est la même cause pour les `Timestamp`/`Date`/`ZonedDatetime` typés, les listes et les octets. **Non corrigé** : il faut une clé d'index pour ces valeurs (voir `docs/LIMITES-v0.0.1.md`).
- **Écrivains parallèles, WAL durable** (fsync à chaque commit, 300 `CREATE` par fil, débit total en `CREATE`/s) :

| config | 1 fil | 4 fils | 8 fils |
|---|---|---|---|
| sans contrainte | 227 | 460 | 878 |
| `UNIQUE (a)` | 217 | 426 | 903 |

  Le verrou d'unicité (un mutex du processus, `GraphStoreMut::merge_guard`) ne se voit pas : le débit suit celui du fsync et croît avec le nombre de fils, avec ou sans contrainte (à 8 fils, 903 contre 878 : dans le bruit). Il n'est pas tenu pendant le fsync du commit.

### VP-Tree `range_search` vs balayage, 2D géo — machine `dgx` (aarch64 GB10, 20 cœurs), 2026-11-21 13:26–13:44, base `52f093c9` + test/fix/bench

But : trancher « à partir de quel n et de quelle sélectivité un index VP-Tree pour un géofence bat un balayage ». Banc `index_bench` (`vp_tree_range_2d`, `balayage_2d`, `vp_tree_build_2d`), lancé par `cargo bench -p obrain-core --bench index_bench --features vector-index -- _2d`.

Trois distributions 2D sur [0,1]² : `uniform` (référence), `quantized` (coordonnées arrondies à 3 décimales — la donnée du pont d'appareil), `duplicates` (50 % des points à la même coordonnée). Rayons 0,01 / 0,05 / 0,15 ; pour une distribution uniforme ils couvrent ≈ 0,03 % / 0,8 % / 7 % des points. Requête au centre (0,5 ; 0,5). **Machine `dgx`** (aarch64 GB10, 20 cœurs), charge début **2,11** → fin **3,92** ; lancé à la main sur dgx parce que le `m4max` était saturé (load ~136) et le swap bloqué à 98 %. `dgx` a cloné `main` (`52f093c9`) et appliqué les 3 commits (test / fix / bench) — mêmes binaires qu'au dépôt, machine différente.

**Lecture du rapport** : `balayage / range_search` (> 1 = le VP-Tree gagne). Distribution **quantized** (le cas réel d'un géofence) :

| n | r=0,01 (≈0,03 %) | r=0,05 (≈0,8 %) | r=0,15 (≈7 %) |
|---|---|---|---|
| 10³ | 17,2× | 3,5× | 0,85× (balayage) |
| 10⁴ | 114,5× | 10,2× | 2,0× |
| 10⁵ | 182× | 27,8× | 4,1× |
| 10⁶ | **894×** | 58× | 5,7× |

Distribution **uniform** (référence) : mêmes ordres de grandeur — 10⁶/0,01 : **953×**, 10⁶/0,05 : 70×, 10⁶/0,15 : 5,9×.

Distribution **duplicates** (pire cas d'un VP-Tree) : l'arbre dégénère (l'élagage ne coupe rien), le balayage gagne partout — 0,26× à 1,10× selon n.

Coût de construction (`vp_tree_build_2d`, quantized) : 10³ **197 µs**, 10⁴ **2,88 ms**, 10⁵ **41,3 ms**, 10⁶ **648 ms**.

**Conclusions.** (1) L'optimisation affirmée tient pour un géofence sélectif : à 10⁶ points et 0,03 % de sélectivité, l'index bat le balayage de **~900×** (1,82 µs contre 1,625 ms). (2) La bascule est à la sélectivité et à n : pour n ≥ 10⁴ l'index gagne jusqu'à ~7 % ; à n = 10³ le balayage gagne au-delà de ~7 % (607 ns contre 518 ns). (3) Sur des données dégénérées (duplicatas massifs) l'index ne sert à rien et coûte plus cher qu'un balayage. (4) Le coût de construction (648 ms pour 10⁶) s'amortit : à 0,03 % de sélectivité, chaque requête économise ~1,62 ms, donc ~400 requêtes suffisent à le résorber.

**Réserve.** Les chiffres sont `dgx`, pas `m4max` (le plan visait m4max). La comparaison *relative* (index/balayage, par n et sélectivité) est valable ; l'absolu est propre à dgx. Voir `crates/obrain-core/tests/vp_tree_range.rs` pour le verrou de correction (défaut d'élagage aux ex æquo, corrigé) qui garantit que ces mesures ne portent pas sur un `range_search` faux.
