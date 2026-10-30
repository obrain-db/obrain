# Chiffres — améliorations mesurées (avant → après)

**Machine n3 · 2026-09-05 · branche feat/zorder.** Références détaillées et
commandes dans `docs/bench-verify/VERIFY-GAINS.md` + ledger.

## Comparatifs multi-variantes (`obrain compare`, stores à props)

| Surface | Départ | Aujourd'hui | Facteur |
|---|---|---|---|
| Gros 443 661 n / 1,67 M arêtes — froid | 7,2 s | ~0,9 s | **×8** |
| Gros — caches nœuds+arêtes (memo off) | (n/a) | **195 ms** | — |
| Gros — un seul côté changé | (n/a) | ~0,7 s | — |
| Gros — mémorisé (rien n'a changé) | (n/a) | **0 ms** | — |
| Paire 35-38 k / ~247 k — froid | 469 ms | **56-85 ms** | ~×6 |
| Paire — chaud (empreintes) | (n/a) | **39-40 ms** | — |
| Paire — mémorisé | (n/a) | **0 ms** | — |
| Coût unitaire (masse) | ~11 µs/nœud·arête | **~0,7-2 µs** | ×5-15 |

## Ingestion & relief

| Opération | Chiffre (n3) |
|---|---|
| `native-import` corpus 445 k/1,67 M | **0,26-0,39 s** moteur (cible ≤ 0,6 s) |
| import code NAPI 37 k (props) | ~3,8 s |
| `enrich --ricci` 1,67 M (copie fraîche) | **~1,8-3,8 s** (écritures WAL 1,67 M dominantes — profilé) |

## Recherches / fenêtres (verify-o1/engine)

`edgeCountByType(CALLS)` **0,16 µs** · `count(n)` **41 µs** · `count(File)` 35,5 µs ·
hubs d≥50 **zruns 71 µs** vs scan 238 µs (exact==sorted) · lookup qn **63 µs** ·
window large exact==sorted ✓.

## Exactitude & robustesse (corrections chiffrées)

| Défaut | Avant | Après |
|---|---|---|
| MATCH non dirigé (`(a)-[:CALLS]-(b)`) | **353 256** (sous-comptage, préfiltre `Both` à tort) | **408 143** correct, sans régression (0,97-1,01 s) |
| Race MERGE+index (`unicite_merge`) | **1-2 échecs / 10** | **0 / 12** (verrou composé, `ced09dba`) |
| Inventaire moteur (141 binaires × 2 passes) | 4 binaires rouges (défaut) | **0 rouge (RC 0 ×2)** |

## Portes (aucune régression)

core **870** · substrate **806** · engine lib **680** (cdc,algos **724**) ·
CLI **65** · engine `--tests` défaut & cdc,algos **141/141 RC 0**.

## Filets ajoutés (propriétés vs oracles indépendants)

compare 40 graines · search_conflicts 30 · search_surprise 10 (égalité <1e-6) ·
fenêtres 2D 250 requêtes · parités de caches (empreintes, arêtes) · memo +
invalidation déterministe.
