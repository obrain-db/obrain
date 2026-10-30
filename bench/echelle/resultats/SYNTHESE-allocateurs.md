# Allocateurs jemalloc / mimalloc / système — T7 (2026-08-30, M4 128 Go)

Binaires : obrain-cli du worktree 599da0f5, liés statiquement via les
features `alloc-jemalloc` (tikv-jemallocator 0.6) et `alloc-mimalloc`
(mimalloc 0.1), exclusives (compile_error sinon) ; défaut = allocateur
système. `MALLOC_ARENA_MAX` : sans objet sur macOS (glibc uniquement) — non
audité ici, à reprendre si un banc Linux apparaît.

## Duel de LOAD ×10 (376 780 nœuds / 2 469 340 arêtes, 3 rondes ALTERNÉES, minima)

| Binaire  | load min (ms) | RSS pic min (Mo) | dispersion observée |
|----------|---------------|------------------|---------------------|
| sys      | 42 110        | 413              | 42,1 → 70,3 s       |
| jemalloc | 33 990        | 413              | 34,0 → 64,5 s       |
| mimalloc | 42 350        | 412              | 42,4 → 56,1 s       |

jemalloc gagne le minimum (−19 % vs sys) mais l'écart est du même ordre que
la dispersion intra-binaire (machine partagée, load I/O-lié) : PREUVE
MONO-RONDE, non confirmée à ×100 (décision orchestrateur : le levier n'est
pas dominant, pas de confirmation à 17 min de load).

**RSS : STRICTEMENT IDENTIQUE (412-413 Mo) aux trois.** La résidence du load
est dominée par les zones mmap file-backed, pas par le tas — l'allocateur
n'y peut rien.

## Requêtes sur la base ×100 (5 passes ALTERNÉES entre binaires, minima, ms / RSS pic Mo)

| Forme | sys | jemalloc | mimalloc |
|-------|-----|----------|----------|
| q1 lookup nom          | 2 431 / 6 950  | 2 323 / 6 998  | 2 318 / 7 021  |
| q2 appelants 1 saut    | 2 752 / 2 966  | 2 556 / 2 982  | 2 397 / 3 027  |
| q3 COUNT label         |   491 / 1 000  |   495 / 1 000  |   533 / 1 000  |
| q4 ORDER BY DESC LIMIT | 3 064 / 2 986  | 2 574 / 2 983  | 2 590 / 3 027  |
| q5 scan filtré         | 6 629 / 15 704 | 6 452 / 15 660 | 5 810 / 15 763 |

## Verdicts

1. **L'allocateur n'est PAS un levier dominant** : gains requêtes de −4 à
   −15 % (mimalloc légèrement devant : q2 −13 %, q5 −12 % ; jemalloc : q4
   −16 %), RSS inchangé partout (±1 %). Le RSS des requêtes vient de la
   charge (q5 : 15,7 Go d'hydratation, q1 : 7 Go), pas du malloc.
2. **Découverte collatérale majeure** : les binaires du worktree 599da0f5
   sont ~9-13× plus rapides que le binaire du banc croisé sur q1/q5 à ×100
   (q1 2,4 s vs 20,9 s ; q5 6,6 s vs 86,0 s) — les correctifs de la vague
   en cours (T1 filter et consorts) ont déjà résorbé une grande part du
   décrochage mesuré sur le tronc du 30/08 matin.
3. Reco : rester sur l'allocateur système par défaut ; mimalloc en option
   de build si un profil de requêtes lourdes le justifie un jour.
