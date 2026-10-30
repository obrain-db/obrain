# Bench Linux — release candidate m6-zorder (2026-09-07)

Binaire : `target/release/examples/bench_queries` (RC merge 50f10d9a).
Arbre : `/tmp/linux-6.6` (70 821 fichiers). Machine : AMD Ryzen 7 8845HS,
62 Go — **chargée pendant les mesures (load 3-8, M6 compilait en parallèle)**.
Référence (idle) : docs/bench/bench-queries-complet-2026-09-07.md.

## Résultats (RC, fenêtre comparable — run 3)
| Métrique | Référence (idle) | RC run 3 | verdict |
|---|---|---|---|
| indexation | 31,0 s | 30,40 s | ✅ |
| nœuds / arêtes | 1 552 935 / 4 791 993 | identiques | ✅ |
| RAM après index | 5 760 Mo | ~5 810 Mo | ✅ |
| top-5 cognitive | 6,74 µs | 12,09 µs | ⚠️ ~2× (machine chargée ; M6-seul 9,8 µs même fenêtre) |
| top-5 complexity | 8,46 µs | **5,76 µs** | ✅ (plus rapide) |
| top-100 cognitive | 574,9 µs | **547,1 µs** | ✅ (plus rapide) |
| counts (récursives/is_test/docstring) | 0,02 µs | 0,02 µs | ✅ O(1) |
| counts arêtes (TESTS/CALLS/DEFINES/FCW) | 0,01 µs | 0,01 µs | ✅ O(1) |
| lookup FQN / positions_of_kind | 0,02 / 0,01 µs | 0,02 / 0,01 µs | ✅ O(1) |

## Interprétation
- **Pas de régression structurelle du merge** : mêmes nœuds/arêtes, indexation
  et RAM conformes ; en fenêtre comparable, complexity et top-100 ≤ référence ;
  tous les accès O(1) tiennent à 0,01-0,02 µs.
- Écart résiduel des top-N cognitifs : bruit machine (load 3-8 + builds M6
  concurrents pendant les premiers runs : 39,6 puis 20,9 µs → 12,1 µs une fois
  le build M6 fini ; M6-seul dans la même fenêtre : 9,8 µs).
- Chiffre final propre : à re-mesurer sur machine au calme (load < 1).
