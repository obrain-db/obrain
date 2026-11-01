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
`release`, Criterion par défaut (100 échantillons, ou la configuration du
bench). Toute mesure prise avec une charge > 2 est rejetée.

## Commandes

```bash
make bench-v001            # les quatre benchs ci-dessous, sorties dans target/criterion
cargo bench -p obrain-substrate --bench tiered_scan        # retrieval vectoriel par paliers (10⁶ nœuds)
cargo bench -p obrain-substrate --bench wal_append         # débit d'append WAL
cargo bench -p obrain-engine    --bench query_bench        # requêtes GQL élémentaires
cargo bench -p obrain-engine    --bench phase_a_write_goulot  # chemin d'écriture, commit scellé (D-DUR)
```

## Chiffres (à produire — V1-PERF)

| Bench | Mesure | Valeur | Machine | Date | Charge |
|---|---|---|---|---|---|
| `tiered_scan` | p95 top-10, 10⁶ nœuds, mono-thread | — | — | — | — |
| `wal_append` | records/s, lot de 1 | — | — | — | — |
| `query_bench` | MATCH simple, µs/requête | — | — | — | — |
| `phase_a_write_goulot` | commits/s, `EveryCommit` (fsync par commit) | — | — | — | — |

## Ce qui n'est PAS reproductible depuis ce dépôt

- Le tableau LDBC SNB (Obrain vs Kuzu, FalkorDB, Memgraph, Neo4j, ArangoDB)
  vient de l'outil externe `graph-bench`, non versionné ici, sans commit ni
  matériel indiqués. Il est retiré du README tant qu'il n'est pas rejoué avec
  une version épinglée de `graph-bench`.
- `docs/ecosystem/performance.md` (« ~2M ops/s », Ryzen 9 5900X) : aucun
  bench du dépôt ne produit ces lignes.
- `crates/obrain-substrate/benches/REPORT.md` (avril 2026) cite un bench
  `crud_micro` qui n'existe plus.
