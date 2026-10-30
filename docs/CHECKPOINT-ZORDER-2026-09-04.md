# Checkpoint versionné — branche `feat/zorder` (2026-09-04)

État consolidé, taggé et QC-vert, prêt à consommer par **aletheia** (binding NAPI + engine)
et le **DSH harness / graph-writer** (binaire `obrain` bridge/mcp + engine).

## Tag & commits
- Tag : `v0.0.1-zorder-2026-09-04` (annoté) — HEAD `5ae779d2` (arbre propre).
- Historique : RFC → primitives Z (hilbert/zorder/zgrid) → `window_query_2d`(+sorted/zruns) →
  chantiers (feature_epoch, prefetch) → engine+NAPI (`windowQuery2d`, `windowQuery2dSorted`,
  `createEdgesBatch(Fast)`, `createNodeNoWal`, `bulkImport`, `edgeCountByType`,
  `nodeCountByLabel`, `createPropertyIndex`, `checkpoint`) → import rapide & ingestion native
  zone-direct (`obrain native-import`, 0,31 s pour 445 k/1,67 M) → tests sémantiques (BM25/
  vector/hybrid) → RFC §12-16 (scorecard, avances ×N, comparatifs kuzu/cbm).

## QC (gate de ce checkpoint) — TOUT RE-MESURÉ à ce HEAD
- `cargo test -p obrain-substrate --lib` : **798/798** verts ; `cargo test -p obrain-engine --lib` : **678/678** verts.
- Builds release : `obrain-cli`, `obrain-node` OK.
- **Kit de vérification commité** (`scripts/bench-verify/`, sorties `docs/bench-verify/2026-09-04-*.txt`) :
  GQL count(n) 50 µs · count(File) 34 µs · count(CALLS) scan 20,2 ms · O(1) 0,18/0,13 µs ·
  fenêtres exact==sorted ✓ (zruns 80 µs vs scan 270 µs) · lookup index 43 µs ·
  **native-import 0,27 s** (445 k/1,67 M) vs kuzu LOAD 0,5 s · QC réouvert exact
  (445 493/1 670 005, CALLS 349 289, File 27 501, Method 41 950).
- Syntaxes GQL exactes & commandes de reproduction : `docs/bench-verify/README.md`.

## Mise à jour ALETHIA (binding NAPI + engine)
1. Dépendances Rust → ce tag (ou branche) : `git = "<repo>", tag = "v0.0.1-zorder-2026-09-04"`
   (ou chemin + checkout du tag) pour `obrain-engine`/crates partagés.
2. Binding : reconstruire l'addon depuis `crates/bindings/node` :
   `cargo build --release -p obrain-node` puis copier `target/release/libobrain_node.so`
   là où l'application le charge (module préchargé / rtprobe).
3. Nouvelles API JS exposées (index.d.ts à jour) : `windowQuery2dSorted`, `createEdgesBatch`,
   `createEdgesBatchFast`, `createNodeNoWal`, `bulkImport`, `edgeCountByType`,
   `nodeCountByLabel`, `createPropertyIndex`, `checkpoint`.
4. Nouveau GQL/usage de fenêtres 2D triées : RFC `zorder-superposition.md` §D3b (contrat : layout
   trié invariant — aucune mutation de features entre `repermuter_par_hilbert` et la fenêtre ;
   repli scan automatique sinon).

## Mise à jour HARNESS (DSH graph-writer, binaire obrain bridge/mcp)
1. Remplacer le binaire `obrain` par la release de ce tag (`cargo build --release -p obrain-cli`).
2. `obrain mcp`/`bridge` : protocole inchangé (RO par défaut) — les stores sessions existants
   restent lisibles (format `SUBSTRATE_FORMAT_VERSION=1` inchangé ; zones Z/zgrid ADDITIVES).
3. Nouvelle base réelle de bench sous `target/bench/real/` : `code-all3.obrain` (triée),
   `code-kuzu.obrain`, store natif CLI (QC exact).

## Compatibilité / format
- `SUBSTRATE_FORMAT_VERSION` inchangé (1) : zones ajoutées seulement (`substrate.zorder`,
  `substrate.zgrid`, colonnes degrés par type) — ouverture des stores existants sans migration.
- Ingestion native zone-direct = store NEUF uniquement (pas de WAL par op, checkpoint final).

## PROD basculée sur le FULL obrain-db (2026-09-04 13:10 UTC)
- `runtime-full.obrain` régénéré depuis `dsh-runtime.db` (source de vérité) via
  `scripts/runtime-to-substrate.mjs` : **256 757 nœuds / 252 146 arêtes en 34 s**
  (644 Sessions / 63 053 Messages / 65 012 ToolCall … — superset des anciens stores
  partiels 142/196 sessions). Textes des Messages issus de `runtime_payloads`
  (fidélité vérifiée : session_load renvoie messages+texte ; runtime_search OK).
- Bascule : drop-in `99b-obrain-runtime-full.conf` (`OBRAIN_STORE=runtime-full.obrain`,
  prioritaire sur 99-…) + `systemctl --user restart dsh-web` → :3082 **HTTP 200**,
  service actif, bridges sous la référence. ROLLBACK : retirer 99b + restart.
- Anciens stores intacts : `runtime.obrain` (prod), `runtime-dev.obrain` (dev).
- Constat : le message « codebase-memory-mcp: secure daemon endpoint could not be
  created » est préexistant/bénin (présent avant bascule) ; chargement non instantané = warmup.

## Déployé comme RÉFÉRENCE harness (2026-09-04)
- Binaire partagé `~/.local/share/obrain-harness/bin/obrain` ← checkpoint (sha256 `be5b2bd390b31b18…`),
  backup `obrain.bak-20260904`, copie versionnée `obrain-v0.0.1-zorder-2026-09-04`.
- Sonde protocole bridge ✓ (initialize + `tools/call sessions_list` → 77 sessions sur copie rt-prod).
- :3082 (prod) NON redémarré (R7) : les bridges lancés gardent l'ancien ; les NOUVEAUX spawns
  (prod & dev) utilisent la référence. Rollback : `cp obrain.bak-20260904 obrain`.
- Redéploiement idempotent : `scripts/deploy-harness-obrain.sh [release] [store-copie]`.

## Références
- RFC : `docs/rfc/substrate/zorder-superposition.md` (§1-§17) — scorecard datée, comparatifs,
  innovations par combinaison, runbook déploiement §9.4 dans `docs/PROTOCOLE-DEPLOIEMENT.md`.
- Tous les chiffres sont datés (machine n3, 2026-09-04).
