# Kit de vérification — checkpoint zorder (2026-09-04)

Reproduit les performances annoncées (RFC §14/§16) sur la machine n3, HEAD `5ae779d2`.

## Commandes
```bash
# 1. Compteurs O(1) — protocole canonique (process dédié, N=20000)
node scripts/bench-verify/verify-o1.mjs
# 2. Requêtes GQL (syntaxes exactes affichées), fenêtres 2D, lookup
node scripts/bench-verify/verify-engine.mjs
# 3. Ingestion native + baseline kuzu (python venv /tmp/benchvenv)
obrain native-import <store-neuf> <corpus.bin>          # corpus via scripts/code-corpus-to-binary.mjs
python /tmp/kuzu_cmp.py                                  # même corpus (CSV)
# 4. Tests sémantiques (BM25/vector/hybrid) — features combinées
cargo test -p obrain-engine --features text-index,vector-index,hybrid-search \
  --test semantic_bm25_smoke --test vector_latency_scale
```

## Sorties capturées (docs/bench-verify/2026-09-04-*.txt)
| Annonce | Vérifié (re-mesure) | Fichier |
|---|---|---|
| edgeCountByType(CALLS) O(1) 0,20 µs | **0,18 µs** | 2026-09-04-o1.txt |
| nodeCountByLabel(File) O(1) 0,14 µs | **0,13 µs** | 2026-09-04-o1.txt |
| count(n) ~54 µs · count(File) ~39 µs | 50 µs · 34 µs | 2026-09-04-engine.txt |
| count(CALLS) scan ~20 ms | 20,2 ms | 2026-09-04-engine.txt |
| GQL count exacts | 445 493 / 27 501 / 95 386 / 349 289 | 2026-09-04-engine.txt |
| fenêtres exact==sorted + zruns hubs | égalité ✓ · zruns 80 µs vs scan 270 µs | 2026-09-04-engine.txt |
| lookup qualified_name (index) ~65 µs | **43 µs** | 2026-09-04-engine.txt |
| native-import 445 k/1,67 M <0,5 s | **0,27 s (wall 0,40 s)** | 2026-09-04-ingest.txt |
| kuzu LOAD (même corpus) 0,5 s | 0,5 s · count(File) 3,47 ms · count(CALLS) 21,6 ms | 2026-09-04-ingest.txt |

Les stores de référence vivent sous `target/bench/real/` (code-all3 triée, code-all props+index,
code-kuzu, store natif). `SUBSTRATE_FORMAT_VERSION=1` inchangé (zones additives).
