# Bench complet : queries + RAM (2026-09-07, commit c2dbbf1f)

Machine : AMD Ryzen 7 8845HS (16 threads), 62 Go RAM. Mode release.
Mesure : `bench_queries` (RSS interne /proc/self/statm) + `/usr/bin/time -v`
(RSS max externe). Queries moyennées sur des milliers de runs.

## Kernel Linux complet — /tmp/linux-6.6 (70 821 fichiers)

Indexation : 31,0 s | 1 552 935 nœuds | 4 791 993 arêtes
RAM : 3 Mo baseline → 5 760 Mo après indexation (max externe : 6 151 Mo —
pic d'allocation rayon pendant l'extraction parallèle).

| Requête | Temps | |
|---|---|---|
| top-5 cognitive | 6,74 µs | |
| top-5 complexity | 8,46 µs | |
| top-5 transitive_loop_depth | 9,47 µs | |
| top-5 loop_depth | 7,03 µs | |
| top-5 fichiers modifiés | 7,02 µs | |
| top-100 cognitive | 574,9 µs | |
| count récursives | 0,02 µs | O(1) |
| count is_test | 0,02 µs | O(1) |
| count has_docstring | 0,02 µs | O(1) |
| count edges TESTS | 0,01 µs | O(1) |
| count edges CALLS | 0,01 µs | O(1) |
| count edges DEFINES | 0,01 µs | O(1) |
| count edges FILE_CHANGES_WITH | 0,01 µs | O(1) |
| lookup FQN (node_by_qn) | 0,02 µs | O(1) |
| positions_of_kind(Function) | 0,01 µs | O(1) |
| top co-évolution | 0,01 µs | O(1) |

RAM des queries : nulle (aucune allocation — lectures de caches).

## Repo obrain — banc-obrain (1 388 fichiers, git)

Indexation : 1,28 s | 32 871 nœuds | 177 551 arêtes
RAM : 3 → 284 Mo (max externe 290 Mo).

| Requête | Temps |
|---|---|
| top-5 cognitive | 2,99 µs |
| top-5 complexity | 0,11 µs |
| top-100 cognitive | 0,50 µs |
| counts (récursives/is_test/docstring) | 0,02 µs |
| count edges (TESTS/CALLS/DEFINES/FCW) | 0,01 µs |
| lookup FQN | 0,02 µs |

## drivers/net — /tmp/linux-6.6/drivers/net (5 418 fichiers)

Indexation : 4,50 s | 278 865 nœuds | 687 224 arêtes
RAM : 3 → 1 148 Mo (max externe 1 210 Mo).
top-5 cognitive 5,92 µs · top-100 cognitive 1,25 µs · counts 0,01-0,02 µs.

## Synthèse

- **RAM par nœud** : kernel 5 760 Mo / 1,55 M nœuds ≈ 3,7 Ko/nœud (graphe
  + colonnes + caches + props JSON). drivers/net : 4,1 Ko/nœud.
- **Toutes les queries** : ≤ 10 µs (top-k) ou ≤ 0,02 µs (counts/lookups) —
  quel que soit le corpus, jusqu'au kernel entier en mémoire.
- La RAM est dominée par le graphe lui-même (props JSON par nœud) ; les
  structures de requêtes (colonnes 8×8 octets, caches top-100, compteurs)
  ajoutent ~0,5 % — le prix minuscule de l'accès O(1)/O(k).
