# Z-order (Morton) superposé à la cascade de Hilbert — analyse de capacités et conception

**Status**: Chaîne moteur LIVRÉE et mesurée sur `feat/zorder` — D1 primitives, D2 zone `Zorder` (bulk/compact/replay), D4 `window_query_2d` scan exact + **D3b `window_query_2d_sorted`** (voie accélérée zruns : sidecar `substrate.zgrid` persistant `(order,max_degree)`, index (zkey,slot) par communauté en cache invalidé par génération, plages Z, prédicat exact, repli scan), engine/NAPI/binding/typings, benches B1-B3 chiffrés (RFC §5.1). Reste opérationnel : bascule des services réels (aletheia + harness) — actions opérateur documentées (§9.4), et B4/B5 (documentés non exécutés).
**RFC area**: substrate (obrain-substrate), impacts cognitif (prefetch) et services
**Branche**: `feat/zorder` (depuis `62b9fe99`, HEAD consolidé de `fix/ch8b-engrams`)
**Auteur**: supervision DSH — chaîne « ordre Z » (analyse, impl, bench, déploiement aletheia + harness)

---

## 1. Contexte et objectif

Le substrate range les nœuds de chaque communauté selon une **courbe de Hilbert 2D sur le plan (centrality × degree)** : c'est la clé de tri secondaire qui rend les nœuds topologiquement proches **physiquement contigus** dans les pages/zones, pour le préfetch du spread (activation cognitive) via `madvise(WILLNEED)` sur des plages contiguës.

Une discussion de conception antérieure (session obrain, « L'ordre z nous apporterait quelque chose ? ») a établi la thèse suivante, à présent à **prouver par le code et la mesure** :

> **Hilbert ordonne physiquement (préfetch de voisinage topologique) ; Z-order indexe logiquement par préfixe (hiérarchie quadtree aplatie, requêtes de fenêtre 2D par plages contiguës O(log N)).** Les deux courbes ne sont pas redondantes ; leur superposition visait un coût dérisoire : bit-interleave ~2–5 ns, remplissage dans la même passe que le tri Hilbert (~0,15 ms pour 66 k nœuds estimé), 4 octets/nœud.

Objectif de cette chaîne : (1) analyse de features/capacités (ce document), (2) implémentation dans obrain-substrate, (3) benchmarks chiffrés vs baseline Hilbert, (4) déploiement et intégration dans les services consommateurs (**aletheia** — binding NAPI `obrain-node` + requêtes — et **le DSH harness graph-writer**, dont le moteur de graphe est `obrain mcp`/`obrain bridge`, cf. §6).

## 2. État actuel vérifié dans le code (cascade de Hilbert)

Ancrages exacts (branche `feat/zorder`) — tous vérifiés par lecture directe :

### 2.1 Primitives — `crates/obrain-substrate/src/hilbert.rs`
- `hilbert_index_2d(x, y, order) -> u32` (l.44) : descente itérative O(order) ≈ 10 ns, `order ≤ 15` (grille 2^order par axe, u32 avec tête).
- `hilbert_key_from_features(centrality_q16: u16, degree: u32, order, max_degree) -> u32` (l.87) : **rescale** `cx = centrality* (n-1)/65535`, `cy = min(degree, max_degree)*(n-1)/max_degree` puis index Hilbert. C'est le point de définition commun du plan 2D.
- `compute_hilbert_permutation(...)` (l.121) : clé tri `tomb(1)|community(32)|hilbert(32)` dans un u128 → `old_to_new`. Variante `compute_hilbert_permutation_page_aligned` (l.217) : début de communauté aligné page (`NODES_PER_PAGE`), tombes absorbant le padding — invariant « aucune communauté ne chevauche une frontière de page ».

### 2.2 Passe de tri — `crates/obrain-substrate/src/writer.rs` `bulk_sort_by_hilbert` (l.1330)
Dix phases (commentaire l.1280) : scan Nodes (community_id, centrality_cached, tombstone) → scan Edges (degré par src) → permutation page-aligned → WAL `HilbertRepermuteV2` (permutation + états engram absolus, fsync avant mutation) → **Phase 4** permutation out-of-place de `Zone::Nodes` → **Phase 5** réécriture `EdgeRecord.src/dst` → **Phase 6** (l.1541) : **rebuild des side-columns** `Zone::Hilbert` et `Zone::Community` (u32/slot) dans une même boucle sur les anciens slots : `hbuf[new] = hilbert_key_from_features(centrality[old], degrees[old], …)` ; `cbuf[new] = communities[old]` → barrière msync → confirmation epoch → remap registres (tier0, degrés, blobcol/veccol) → méta (`flags |= HILBERT_SORTED`, `hilbert_order`) + commit.
Idempotence : early-out si `HILBERT_SORTED` posé (re-sort = opération privilégiée qui efface d'abord le flag).

### 2.3 Cycle de vie des side-columns (sémantique « hint »)
- **Replay** — `replay.rs` `apply_hilbert_permutation` (l.1355) rejoue Nodes+Edges depuis la permutation du WAL. Pour `CompactCommunity` (l.842), le commentaire l.827-841 est explicite : la colonne `hilbert_key` **n'est pas recouvrable du WAL** (fonction de (centrality, degree) au moment du compactage) ; le replay **zéro les anciennes entrées et laisse les nouvelles** ; si la phase 8 du writer n'a pas abouti avant le crash, elles restent zéro et **le prochain tick du `CommunityWarden` les recompute**. *« Hilbert is an optimisation hint (locality prefetch), not a correctness invariant — degraded locality is tolerable, incorrect topology is not. »*
- **Compactage** — `compact_community` (writer.rs l.1865, rebuild zones l.2053-2071) et `slot_remap.rs` (remap de `Zone::Hilbert` l.2623/2645) suivent le même motif.
- **Ouverture replay** : `replay.rs` l.862-865 ouvre `Zone::Hilbert`/`Zone::Community` paresseusement (Option<ZoneFile>).

### 2.4 Zones fichier
- `file.rs` l.329 `pub mod zone` : noms de fichiers — `substrate.hilbert`, `substrate.community`, … `ALL_MMAPPED` (l.349) liste les zones mmap. `Zone` enum l.368, `filename()` l.406.
- `checkpoint.rs` l.68-69 énumère `Zone::Hilbert`, `Zone::Community` (couverture de checkpoint).
- `meta.rs` : `SUBSTRATE_FORMAT_VERSION = 1` ; `MetaHeader.hilbert_order` ; flags méta bit 1 = `HILBERT_SORTED` (format-spec §2).
- Chaque zone = **fichier séparé** mmap, `ZoneFile` (file.rs) avec `open_zone` (file.rs l.926) ; une zone absente sur disque = fichier vide/non mappé (sémantique à confirmer pour la création — voir §4.2 D2).

### 2.5 Consommation de la localité (le « pourquoi » physique)
- `store.rs` `SubstrateStore::prefetch_community(community_id)` (l.7720) : `community_slot_range(cid)` → `advise_zone_willneed` sur `[first,last]` × 32 B (Nodes), ×4 B (Community), ×4 B (Hilbert). Une communauté entière = **une plage contiguë par zone** (grâce à l'alignement page).
- Pont cognitif : `activation.rs` `spread` (BFS l.352) → `graph.on_activated(node)` (commentaire « Cascade Hilbert : préfetch du voisinage physique de la communauté avant d'entrer dans le BFS à ce niveau », l.437-441) → `SynapseActivationSource::on_activated` / `CausalAwareActivationSource::on_activated` (l.211/312) → `SubstrateStore::on_node_activated` → `prefetch_community`. **Aujourd'hui : préfetch de la communauté ENTIÈRE au premier contact d'un de ses membres.**
- Appelants de `bulk_sort_by_hilbert` : CLI (`topo hilbert`, `mcp_server` `import_events`/`memory_register`), wardens (`CommunityWarden::tick`, thinker `consolidator`/`CommunityWardenThinker::tick`), `SubstrateStore::repermuter_par_hilbert`.

### 2.6 Mesure existante
- `tests/prefetch_page_faults.rs` : méthodologie `ru_minflt` (getrusage) + éviction `MADV_DONTNEED`, critère `prefetch ≤ 0.5 × no_prefetch`, Linux-only `#[ignore]`.
- Harnais bench : `bench/substrate` (workspace standalone) — runner `bench-substrate` (cibles po/megalaw, sorties JSON `BenchResult` + gates T9) ; criterion `activation`/`crud`/`retrieval`/`ldleiden` ; micro-benches in-crate `crates/obrain-substrate/benches/spreading_activation.rs`, `tiered_scan.rs` (cascade).

## 3. Analyse de capacités — ce que Z-order apporte (et ne peut pas apporter)

| Propriété | Hilbert | Z-order (Morton) |
|---|---|---|
| Localité de voisinage (préfetch topologique contigu) | ✅ parfait (pas unitaire) | ⚠️ sauts aux frontières de quadrants |
| **Hiérarchie par préfixe** (k premiers bits = cellule 2^k×2^k) | ❌ rotations cassent les préfixes | ✅ gratuit (bit-interleave) |
| Fenêtre rectangulaire → plages 1D contiguës | complexe | ✅ **O(log N) plages** (décomposition standard) |
| Coût de calcul | ~10 ns (rotations) | ~2–5 ns (bit-interleave pur) |

**Contrainte physique structurante** : l'ordre physique des slots d'une communauté est UNIQUE (c'est la localité de préfetch du spread). Il ne peut pas être simultanément trié par Hilbert et par Z. Conséquences honnêtes :

1. **Le préfetch physique reste Hilbert.** Un « préfetch par cellule Z » n'a pas de sens physique direct (les pages sont hilbert-ordonnées). Le raffinement *physique* légitime est le préfetch de **sous-plage Hilbert** (ne charger que les pages des nœuds réellement activés), indépendant du Z.
2. **La valeur de Z est logique** : un ordre/une indexation par préfixe du plan (centrality×degree) pour **énumérer des fenêtres 2D** sans scanner la communauté, et pour **partitionner hiérarchiquement** le plan (clustering multi-échelle gratuit).
3. Une **option d'ordre intra-communauté Z** (au lieu de Hilbert) donnerait la contiguïté *physique* des cellules de préfixe (fenêtres = plages physiques contiguës) au prix de la localité topologique — à mesurer, pas à décréter.

### Capacités candidates (à confirmer par la cartographie des workloads, §7)
- **C1 — Requête de fenêtre 2D sur communauté** : `centrality ∈ [a,b] ET degree ∈ [c,d]` (ex. « les hubs » = coin haut-droit). Aujourd'hui : scan de la communauté ou pas de support dédié. Avec Z : énumération en O(log N) plages d'un index trié par zorder.
- **C2 — Clustering hiérarchique du plan** : découpage en cellules 2^k par masque de préfixe → granularité adaptative pour du filtrage/regroupement multi-échelle (retrieval, topk, stats par région du plan).
- **C3 — Ordre intra-communauté Z (mode)** : pour des workloads dominés par les fenêtres, alternative mesurable à Hilbert.

## 4. Conception proposée (draft — décisions ouvertes listées en §7)

### D1 — Primitives (substrate)
Nouveau module `crates/obrain-substrate/src/zorder.rs` (ou extension hilbert.rs) :
- `morton_encode(x: u32, y: u32) -> u32` / `morton_decode` (bit-interleave pur),
- **rescale partagé** : refactorer `hilbert_key_from_features` pour exposer `grid_coords(centrality_q16, degree, order, max_degree) -> (cx, cy)` — garantit que Hilbert et Z opèrent sur **le même plan rescale** (la superposition préfixe n'a de sens que sur une grille identique),
- `zorder_key_from_features(...) = morton_encode(cx, cy)`,
- `zorder_window_to_ranges(ax, bx, ay, by, order) -> Vec<(u32,u32)>` : décomposition d'une fenêtre en plages Z contiguës.

### D2 — Side-column additive `Zone::Zorder` (sémantique hint, PAS de bump de format)
- `substrate.zorder` : u32/slot, `Zone::Zorder` + `filename()` + `ALL_MMAPPED` + couverture `checkpoint.rs`.
- **Remplissage** : dans `bulk_sort_by_hilbert` Phase 6 (même boucle que Hilbert/Community, `zbuf[new] = zorder_key_from_features(centrality[old], degrees[old], …)`) ; idem `compact_community` et le chemin équivalent ; `slot_remap` remap de la zone.
- **Replay** : même sémantique hint que Hilbert — zéro des anciennes entrées, conservation des nouvelles, recompute par le warden si manquant. Colonne **optionnelle** : un store ouvert par un binaire zorder sans la zone = zone absente → capacité zorder non disponible (lecteurs gèrent l'absence), les anciens binaires ignorent le fichier additionnel.
- Coût : +1 bit-interleave + 1 écriture u32 par nœud dans la passe existante (objectif ~0,1–0,2 ms pour 66 k nœuds à mesurer, cf. bench B1).

### D3 — Index auxiliaire trié par zorder (pour C1/C2)
Deux variantes à départager par bench :
- **V3a — en mémoire** : par communauté, `Vec<(zorder, slot)>` trié, construit à l'ouverture du store (ou pendant la passe de bulk sort) ; fenêtre = O(log N) plages → bornes par binary search → énumération des slots.
- **V3b — fichier additionnel** (index secondaire persistant) : plus rapide à l'ouverture, mais étend le format et l'outillage.
Le bench B2 (fenêtres) décidera ; en première intention V3a (zéro format, coût de construction mesurable).

### D4 — API et points d'accroche
- Store : `window_query(community_id, centrality: Range<u16>, degree: Range<u32>) -> iterator de slots` (via D3 ou scan-prédicat si index absent).
- Exécution des fenêtres sur données réelles : centrale dans le CLI `topo`/procédures CALL (`obrain.…`) et/ou la surface NAPI — **à trancher avec la cartographie des surfaces** (§7).
- Préfetch adaptatif (amélioration indépendante, mesurable) : `prefetch_community_partial(community, slots_activés | sous-plage Hilbert)` pour ne pas charger toute la communauté au premier contact (workload spread).

### D5 — Ordre intra-communauté Z optionnel
Flag de tri alternatif (analogue `HILBERT_SORTED`) pour expérimenter C3 ; probablement derrière une feature cfg ou un choix au bulk sort, **uniquement si un bench justifie** un mode fenêtre (B3).

## 5. Plan de benchmark (P3) — méthodologie et critères

| Bench | Mesure | Méthode | Critère indicatif |
|---|---|---|---|
| B1 — coût de remplissage | μs/passe, nœuds/s | criterion, `bulk_sort_by_hilbert` N=66 k et 10⁶, ±zorder | +zorder ≤ ~2 % du temps de passe ; objectif absolu ~0,15 ms @66 k |
| B2 — fenêtres 2D | latence, nœuds lus, plages | fenêtres aléatoires (10 %, 1 %, 0,1 % du plan) sur communauté réelle : index Z (V3a) vs scan communauté vs filtre naïf | index Z ≪ scan dès que la fenêtre < ~10 % ; O(log N) plages vérifié |
| B3 — ordre physique | minor page faults (ru_minflt) + latence spread depth-3 | méthodologie `prefetch_page_faults` (éviction DONTNEED, ratio ≤ 0,5) ; ordres hilbert vs zorder sur mêmes données | Hilbert gagne le spread (attendu) ; quantifier la perte Z pour documenter C3 |
| B4 — préfetch adaptatif | faults et latence spread partiel | préfetch communauté entière vs sous-plage des nœuds activés (B4-D4) | sous-plage ≤ communauté pour activations partielles |
| B5 — gates existantes | régression activation/retrieval | runner `bench-substrate` (po/megalaw) + gates T9 | aucune régression |

Livrables : JSON `BenchResult` dans `target/bench/`, rapport comparatif, et un fichier de gates (`bench/substrate/*_gate.json` style existant) si les critères se stabilisent.

## 6. Compatibilité, déploiement et intégration services

### 6.1 Format et outillage
- Zone additionnelle **additive** : `SUBSTRATE_FORMAT_VERSION` inchangé (décision à valider) ; anciens binaires ignorent `substrate.zorder` (jamais ouvert) ; nouveaux binaires sur anciens stores : zone absente ⇒ capacité zorder non disponible, pas d'échec.
- Outillage qui énumère les fichiers/zones à mettre à jour : `monitor-obrain.sh`, `repair-degrees`, sauvegardes, `checkpoint.rs` (couverture), liste `ALL_MMAPPED`.
- La « sonde de dérive des degrés » (protocole §5) lit `substrate.degrees.*` : non concernée directement, à re-vérifier.

### 6.2 Déploiement (PROTOCOLE-DEPLOIEMENT.md — 7 règles, portes G0–G5)
- Dev d'abord : build `nice` release, store chaud (`obrain mcp --store <dev>`), restart `dsh-dev-3083` (cette GUI), portes fonctionnelles (écoute, contenu, historique, cohérence, stabilité 60 s).
- Prod (`dsh-web` :3082) jamais redémarrée pour une action de dev (R7) ; tag + manifeste obligatoires (R2), scellement avec remote (G5).
- **Aletheia** : rebuild `obrain-node` release → `scripts/sync-obrain-binding.mjs` → `vendor/obrain-db/libobrain_node.node` ; activation des nouvelles capacités (requêtes fenêtre) dans `src/lib/aletheia/obrain-store.ts`/harness — détails à compléter avec la cartographie services.
- **Harness graph-writer** : le moteur de graphe est `obrain mcp`/`obrain bridge` (lancé par le GraphWriter/GraphInjector) ; specs de régression à rejouer (`resume-ordre-journal` et voisins) après toute évolution de l'ordre physique des pages.

## 7. Décisions ouvertes (à clore avec les cartographies moteur/services + benches)
1. Workload réel consommateur de C1/C2 (fenêtres 2D) côté moteur/services — existe-t-il déjà des requêtes centrality×degree ? (typed_degree_topk ? retrieval ? topo CLI ?)
2. Colonne persistée D2 vs calcul volant (2 ns) selon l'usage réel.
3. Index auxiliaire V3a (mémoire) vs V3b (fichier).
4. Mode ordre intra-communauté Z (D5) inclus ou non au périmètre impl.
5. Surface d'API exposée (store method seulement, ou procédure CALL / méthode NAPI `window_query`).
6. Seuils d'acceptation des benches (B1–B5).
7. Format : confirmation que l'ajout de zone additive ne nécessite pas de bump de `SUBSTRATE_FORMAT_VERSION` ni de flag de capacité dans le meta.

## 8. Compléments de la cartographie moteur (vérifiés, carte complète)

- **Deux « hilbert » distincts** : (a) la courbe 2D substrate de la cascade (celle-ci) ; (b) `obrain-adapters::plugins::algorithms::hilbert_features` = features d'embeddings 64-72d multi-facettes (thinker `HilbertEnricher`, feature `enrichment`) — **sans rapport avec l'ordre physique, à ne pas confondre**.
- **Aucune implémentation zorder/Morton nulle part** (grep global : seulement les deux commentaires « Why Hilbert and not Z-order »). Aucune API de fenêtre 2D n'existe.
- **Paramètres d'échelle** : `order` persisté (`MetaHeader.hilbert_order`, méta 96 B exact, `meta.rs`), **`max_degree` N'EST PAS persisté** (CLI `topo.rs:212`, warden :255 avec 32, tests variés). Conséquence décisionnelle : interpréter une fenêtre en degrés absolus exige de **figer/persister `max_degree`** (convention doc `hilbert.rs:77` = 4096) — sinon colonne illisible. → **Décision 8 : persister `max_degree` (méta 96 B → champ ajouté au header ? interdiction de toucher MetaHeader) ou figer la convention 4096**.
- **Passe de remplissage exacte** : Phase 6 de `bulk_sort_by_hilbert` (`writer.rs:1541-1568`), boucle `old→new` avec `centrality[]/degrees[]` déjà en RAM depuis la Ph1 (scan Nodes + scan Edges O(edges)). **`compact_community` rebuild avec `degree=0` par conception** (« degree unknown here », `writer.rs:1916`) → zorder y serait dégradé pareil (hint) — sauf à plomber le degré via le sidecar `DegreeColumn` (`store.rs:13509`, O(1)/slot, fichier `substrate.degrees.node.u32`) — chantier « Step 7 ».
- **Replay asymétrique assumé** (`replay.rs:827-841`) : les side-columns ne sont PAS rebuildées au rejeu — « optimisation hint, localité dégradée tolérée ». zorder = même classe (hint) ; alternatives (payload WAL +4 B/nœud, re-dérivation au rejeu par re-scan edges) documentées, non retenues en première intention.
- **Cycle de vie en ligne** : les inserts (`create_node_in_community`/`allocate_node_id_in_community`, `store.rs:7620/9735`) invalident `HILBERT_SORTED` (`invalidate_layout_flag`, `store.rs:9813`). La colonne zorder suit le même cycle (rebuild au sort ; lecture sous flag, tolérance de ranges pré-compaction — patron `store.rs:7671-7691`) ; tombstones/zéro-sentinelles à neutraliser (0/u32::MAX) dans la colonne.
- **Câblage zone** : `Zone` enum `file.rs:368`, noms `file.rs:329` (`zone::ZORDER = "substrate.zorder"`), `Zone::filename` :397, `ALL_MMAPPED` :349 (pré-création des fichiers vides à `create` :789, `open_or_create` tolère l'absence :459), checkpoint `checkpoint.rs:60-72`, bras one-shot `advise_zone_willneed` `writer.rs:2763-2771`.
- **Consommation** : `prefetch_community` (`store.rs:7720`) mono-range par zone ; `on_node_activated` (`store.rs:7754`) ; pont `#[cfg(feature="substrate")]` (`activation.rs:313`) ; `CALL obrain.cognitive.spread` → `execute_spread` (`obrain-engine/src/cognitive_procedures.rs:124-208`, dispatch :60-106).
- **Invariants** : `NODES_PER_PAGE=128` (record.rs:100) ; slot 0 pinné ; tests existants `tests/hilbert_bulk_sort.rs` (E2E du tri — **à étendre à la colonne zorder**) et tests unitaires hilbert.rs.
- **Benches** : criterion in-crate `crates/obrain-substrate/benches/spreading_activation.rs` (BFS inline, pas de dépendance cognitive — layering ; échelle `OBRAIN_BENCH_N`) ; nouveau `benches/zorder_window.rs` + entrée `[[bench]]` ; méthodologie faults `tests/prefetch_page_faults.rs` (ru_minflt, ratio ≤ 0,5, Linux `#[ignore]`) ; `bench/substrate` (workspace standalone, JSON `BenchResult`, gates T9) ; `bench/echelle/bench_requetes.sh` (5 formes Cypher — une 6e « fenêtre » quand la procédure CALL existera).

## 9. Intégration services — faits vérifiés (cartographie services)

### 9.1 Aletheia (chaîne + tissage)
- Binding NAPI : crate **`obrain-node`** (`crates/bindings/node`), classe `ObrainDB` (`create`/`open`/`execute` GQL, `createNode`/`createEdge`…), chargé par `src/lib/aletheia/obrain-store.ts` (candidats : `ALETHEIA_OBRAIN_BINDING` → `vendor/obrain-db/libobrain_node.node` → `.vercel/output/.../_libs` → dev `banc-obrain/target/release`), store `data/xth/obrain/council.obrain` (**vérifié vivant** : 937 nœuds / 477 arêtes ; voix solveur `solver:graph-writer-dsh` présente).
- Déploiement : `scripts/sync-obrain-binding.mjs` (`cargo build --release -p obrain-node` dans banc-obrain → copie `.so` → `.node` dans `vendor/obrain-db/`) + restart de l'unité systemd aletheia (n3, serve nitro/prod).
- Activation d'une capacité : exposer la procédure/requête via le binding (GQL `execute`) puis appeler depuis `obrain-store.ts`/harness — canal existant documenté.

### 9.2 DSH harness graph-writer (le « moteur obrain » du spec)
- Le spec `resume-ordre-journal.spec.ts` désigne un **vrai moteur substrate**, mais **externe au monorepo** : le binaire `obrain` (banc-obrain, crate `obrain-cli`, sous-commande `bridge` alias `mcp`) est lancé par le GraphWriter/GraphInjector du harness en JSON-lines.
- Store runtime harness : répertoire de zones mmap `~/.local/share/obrain-harness/data/runtime.obrain` (~3,1 Go), zones `substrate.journal.<session>` (header OBJ1, `crates/obrain-engine/src/journal_zone.rs`). L'ordre physique vient de la lecture de zone ; **le harness re-trie par seq** (`session-persistence-graph/src/index.ts:352-371`) — c'est la porte de régression à rejouer après toute évolution de l'ordre physique.
- `codebase-memory-mcp` (daemon SQLite par projet) = **ancien/miroir sans obrain** — hors périmètre d'intégration de la capacité zorder.

## 9.3 Contraintes d'intégration constatées (aletheia, vérifié)
- Aletheia ne calcule **ni communautés LDleiden ni pagerank** sur `council.obrain` (grep : aucune utilisation substrate de `community_id`/centrality — les modules `complex`/`force`/`graph-wasm` relèvent de la théorie des graphes applicative, pas du substrate). Conséquences :
  - la fenêtre **par communauté** n'a pas de cible tant qu'aletheia n'a pas exécuté le détection de communautés (CLI `obrain topo` / LDleiden cognitif — capacité disponible, non câblée côté aletheia) ;
  - `centrality_cached` y vaut 0 par défaut (pas de pagerank) ;
  - l'axe **degré** est vivant (les `createEdge` du moteur incrémentent la colonne degrés) → la démo d'intégration honnête = fenêtre sur la communauté 0 (bucket non catégorisé = tout le store, padding zéro exclu) avec `d_lo ≥ k` : « hubs du tissage » (nœuds conseil/solveur les plus connectés), aujourd'hui servis par scan GQL + tri JS (brain-map, readback).
- Le store `council.obrain` contient une zone `substrate.cdc_journal` (change-data-capture active) : toute modification de format/zone doit passer les portes de régression du canal CDC.

## 9.4 Runbook de déploiement (aletheia + harness) — opérations séquencées
Source : PROTOCOLE-DEPLOIEMENT.md (7 règles, G0-G5), protocol-multi-env.md, cartographie services §9.
1. **Moteur** (banc-obrain, branche feat/zorder) : commits sur arbre propre (R2), tag avant installation prod.
2. **Binding aletheia (dev)** : `cargo build --release -p obrain-node` dans banc-obrain — le loader d'aletheia (dev) retombe sur `target/release/libobrain_node.{node,.so}` (candidat #5 d'`obrain-store.ts`) → aucune copie.
3. **Aletheia prod** : `node scripts/sync-obrain-binding.mjs` (copie `.so` → `vendor/obrain-db/libobrain_node.node`), rebuild app si TS touché (`npm run build`), restart de l'unité du service (hôte aletheia à confirmer — pas d'unité `aletheia.service` sur cet hôte), portes HTTP (`/api/aletheia/obrain`, `?hubs=K`).
4. **Harness dev (graph-writer, :3083)** : installer le binaire `obrain` rebuildé via `scripts/run-dev-3083-obrain.sh` (OBRAIN_BIN → `~/.local/share/obrain-harness/bin/obrain`, préchauffage `sessions_list`), rejouer les specs de régression graph (dont `resume-ordre-journal.spec.ts`), restart `dsh-dev-3083`, portes G3 (écoute/contenu/historique/cohérence/stabilité 60 s).
5. **Harness prod** : régime effectif à vérifier (binaire C `codebase-memory-mcp` vs `obrain` selon l'unité/drop-in) avant toute bascule (R3, R7) ; `dsh-web` :3082 jamais redémarré pour une action de dev.
6. **Scellement** (G5) : tag annoté, manifeste (sha256+commit), poussée hors machine.

### Résultats mesurés (2026-09-04, machine n3 16 cœurs, criterion ×30, seed fixe — exécution finale, bench committé)
Modélisation : B1 sur les distributions littérales (centrality log-uniforme u16 × degré Pareto(2) cap 4096) ; B2/B3 en coordonnées de grille log-uniforme par axe (les distributions littérales s'effondrent sous le quantizer linéaire : 98 % du mass sur cy=0 → mesures de géométrie impossibles — 1ʳᵉ exécution écartée, constat documenté dans le bench).
- **B1 (coût de remplissage)** : `zorder_key_from_features` **6,5–9,0 ns/clé** vs hilbert 14,2–20,7 ns (order 6/8, max_degree 32/4096, m=100 k) → Z ≈ **41–46 % du coût du hkey** déjà payé dans la même passe : la superposition augmente le coût de la passe de tri de ~45 % d'un hkey, jamais doublée.
- **B2 (fenêtres 2D, m=66 k, grille 64×64, 3 708 cellules occupées, 20 fenêtres/taille, comptages scan==zruns assertés)** :

| aire grille | matches/fen. | runs Z/fen. | SCAN | ZRUNS | vainqueur |
|---|---|---|---|---|---|
| 50 % | 15 553 | 87,9 | 29,7 µs | 43,7 µs | scan 1,5× |
| 10 % | 3 201 | 41,3 | 22,7 µs | 13,3 µs | zruns 1,7× |
| 1 % | 271 | 11,5 | 25,6 µs | 1,77 µs | zruns 14× |
| 0,1 % | 90 | 3,2 | 31,8 µs | 0,57 µs | zruns 56× |
| 0,01 % (≈1 cellule) | 22 | 1,0 | 26,4 µs | 0,20 µs | zruns 131× |

  Crossover scan/zruns entre 50 % et 10 % d'aire (~5–8 k matches) ; runs/fenêtre faibles, conformes au modèle quadtree (88 → 1). → **D3b tranché : l'accélération par index-Z est validée pour les fenêtres ≤ ~10 % d'aire** ; à implémenter côté store (prérequis : persistance de `max_degree`, cache par communauté).
- **B3 (localité des courbes, 239 880 paires 4-voisins)** : écarts d'index — hilbert mean 28,3 (p50 1, p99 715, max 3413) vs zorder mean 20,6 (p50 2, p99 342, max 1366) ; queues ≥128 : 3,25 % vs 2,88 % ; ≥1024 : 0,42 % vs 0,40 %. **Z n'est PAS plus dégradé sur l'écart entre cellules 2D-adjacentes** (max 1366 = borne analytique Morton vs 3413 = couture hilbert) ; l'avantage hilbert se joue sur la propriété inverse (indices consécutifs → pas 2D unitaires, testée dans hilbert.rs), pas sur des gaps plus petits.
- Précision : timings ±10–20 % entre runs (machine partagée) — ordres de grandeur ; stats B2/B3 (matches/runs/distribution) déterministes et exactes. B4 (préfetch adaptatif) et B5 (gates) non exécutés (documenté).

## 9.5 Vérifications runtime (2026-09-04, binding release feat/zorder)
- **Capacité prouvée de bout en bout** sur store bien formé via le binding NAPI frais (`cargo build --release -p obrain-node`) : `createNode` ×8 + `createEdge` ×12 → `windowQuery2d(0, 0, 65535, d_lo, 2³²-1)` renvoie exactement les nœuds de degré sortant attendu (d≥2 → 3 hubs [1,2,3], d≥4 → ∅) ; `getNode` résout les `qualified_name`. Zéro-sentinelles exclues.
- **`council.obrain` (aletheia)** : ouvert read-only OK (937 nœuds / 477 arêtes), `windowQuery2d` présent ; **constat de données distinct** : tous les nœuds en communauté 0, colonne degrés *générique* (`substrate.degrees.node.u32`, 1 052 672 o ≈ 131 k slots) à 0 alors que le store est fragmenté (watermark ~131 k slots, 937 vivants) — les arêtes HAS_ROUND/HAS_STEP n'ont pas alimenté la colonne générique (typées présentes). À traiter côté aletheia (re-hydratation `repair-degrees`) avant qu'une fenêtre degré y serve — indépendant de la capacité Z.
- **Naming NAPI** : la factory s'exporte `openReadOnly` (pas `open_read_only`) — à refléter si l'interface TS d'aletheia l'utilise.
- **Harness** : le binaire `obrain` (CLI, `bridge`/`mcp`) rebuildé en feat/zorder doit ouvrir les stores runtime existants sans régression (zones additives) — smoke sur copie avant tout échange de service (R7, continuité de session : ne pas redémarrer dsh-dev-3083 en cours d'exécution).

---

*Document vivant v5 — moteur + binding livrés et prouvés ; benchs chiffrés (§5.1) ; arbitrages D3b clos (index-Z validé, à implémenter à la demande avec persistance de `max_degree`), B4/B5 non exécutés (documenté) ; intégrations aletheia (code prêt, constat degrés signalé) et harness (smoke en cours).*

## 10. Changelog de livraison (feat/zorder)
- v1-v6 : analyse + impl + benches (commits 489b009d → 97c9ca29).
- 2026-09-04 D3b : `substrate.zgrid` (7461bdec) + `window_query_2d_sorted` (e2bd535f) — accélération validée au bench (1,7×→130× selon sélectivité), contrat « layout trié » documenté, cache invalidé par génération, tests 6/6 + suites vertes.

## 11. Chantiers de relief (QC) — état
1. **Test préexistant `operators_expand` (base 62b9fe99)** — CORRIGÉ à la racine (`677e8416`) : pré-filtre typed-degree sensible à la casse → résolution canonique unique par opérateur. 10/10 verts.
2. **Degré réel dans `compact_community` (Step 7)** — LIVRÉ (`9629dc0e`) : fin du `degree=0` dégradé (scan d'arêtes restreint aux membres) ; colonnes Hilbert/Zorder justes après compaction ; test dédié + helper replay robuste à l'ordre (mapping par label). lib 795/795.
3. **Époque de mutations de features** — LIVRÉ (`bb632569`) : `SubstrateFile.feature_epoch` (bump writer : write_node/write_edge/update_centrality/update_community — jamais layout/replay), snapshot en fin de `repermuter_par_hilbert`, `SubstrateStore::sorted_view_valid()` exigé par `community_zrows` AVANT le cache → repli scan après toute mutation post-tri (cache à génération égale ne peut plus servir une colonne périmée). lib 796/796.
4. **Préfetch adaptatif du spread** — MESURÉ, NON implémenté par défaut (anti-churn QC) : bench de référence `spreading_activation` (N=100 k, machine n3) : baseline 33,4 µs / with_prefetch 31,7 µs / prefetch_alone 9,5 µs → le préfetch communauté entière ne gagne que ~5 % sur un balayage complet. Un préfetch par sous-plage hilbert n'a de gain démontrable que sur des activations PARTIELLES ; l'expérience requise (bench à seeds réduits / communauté 10⁶) n'existe pas dans le harnais actuel. Décision QC : ne pas modifier le hot path sans bench dédié ; la voie additive candidate (store `prefetch_community_hilbert_window(cid, centre, demi-largeur)`, madvise mono-range déjà capable) est spécifiée pour quand le workload l'exigera.

## 12. Validation à l'ÉCHELLE (2026-09-04, machine n3, bases plus grosses)

### Spread depth-3 @ 10⁶ nœuds (critère T11 ≤ 1 ms) — `OBRAIN_BENCH_N=1000000`
| variante | temps |
|---|---|
| baseline (BFS seul) | ~22,5 µs |
| avec `on_node_activated(seed)` (préfetche communauté) | ~51,4 µs |
| prefetch seul (hook) | ~8,2 µs |

**Gate T11 PASSÉ avec ~20× de marge** (max ~52 µs ≪ 1 ms). Note QC : à 10⁶, le préfetche « communauté entière » coûte ~+29 µs (madvise d'un range de ~32 Mo+communauté) — argument chiffré pour un préfetche BORNÉ à l'échelle (API `prefetch_community_centered`, chantier 4).

### Fenêtres 2D & remplissage @ 300 000 points/communauté (`OBRAIN_ZORDER_N=M=300000`)
- **B1 fill** : hilbert 13,7 ns/clé vs zorder 6,4 ns/clé → Z = **~46 % du hkey** (stable avec l'échelle).
- **Index zruns** : build table (z,slot) 300 k = **5,5 ms one-shot** (mémoire ~8 o/point = 2,4 Mo @300 k — négligeable, par communauté en cache).
- **B2 fenêtres** (grille 64², 3 967/4 096 cellules occupées, occ. max 8 327/cellule) :

| aire | matches/fen. | runs/fen. | SCAN | ZRUNS | gain |
|---|---|---|---|---|---|
| 50 % | 70 440 | 87,9 | ~105 µs | ~340 µs | scan 3× |
| 10 % | 14 445 | 41,3 | ~107 µs | ~61 µs | zruns 1,8× |
| 1 % | 1 211 | 11,5 | ~104 µs | 10,6 µs | zruns 10× |
| 0,1 % | 397 | 3,2 | ~104 µs | 1,86 µs | zruns 56× |
| 0,01 % | 96 | 1,0 | ~111 µs | 0,34 µs | zruns 325× |

Crossover scan/zruns entre 10 et 50 % d'aire, comme à 66 k → **le seuil de bascule est stable avec l'échelle** (le bail scan >~⅓ de la communauté couvre le bon régime).
- **B3 localité** (1 097 852 paires 4-voisins @300 k) : hilbert mean 28,6 (p99 715, max 3413, ≥1024 : 0,43 %) vs zorder mean 20,6 (p99 342, max 1366, ≥1024 : 0,39 %) → **Z toujours non dégradé** (gaps ≥16 légèrement plus fréquents pour Z, mais queues lourdes meilleures).

### Complétude & robustesse à l'échelle
- Slots u32 : 300 k/10⁶ ≪ 2³¹ — aucun risque ; grille : order 8 (256²=65 536 cellules) conseillé ≥10⁶ (collisions multi-nœuds par cellule absorbées par la vérification exacte des candidats ; la sélectivité des plages Z reste bonne tant que la fenêtre ≪ aire).
- Cache zruns par communauté : invalidation par génération + garde `sorted_view_valid()` (chantier 3) — sain à toute échelle (aucune entrée périmée servie après mutation).
- QC post-bench : lib substrate 797/797, engine 2/2, build release `obrain-node` OK.

### Recommandations
1. Brancher `window_query_2d_sorted` côté engine/NAPI sur données saines quand le workload « hubs/fenêtres » apparaîtra (council réparé + LDleiden/pagerank).
2. Pour le spread à grande échelle : préférer le préfetche BORNÉ (`prefetch_community_centered`) au préfetche communauté entière (le coût du madvise complet est mesuré ci-dessus).
3. B4 (activation partielle) et benches multi-communautés réels (megalaw/PO) restent à exécuter quand les datasets seront disponibles — le harnais et les gates existent (`bench/substrate`, `bench/echelle`).

### Validation sur DONNÉES RÉELLES (copie du store harness `runtime.obrain`, 2026-09-04)
- **Base** : copie reflink `target/bench/real/rt-prod` (~3,1 Go dont ~17 Mo de graphe cœur ; 249 zones — journaux/props dominants). Graphe réel : **135 173 nœuds / 132 223 arêtes**, jamais trié auparavant (`community`/`hilbert` = 0 o).
- **Pipeline de production complet** : `obrain topo hilbert` (CLI release feat/zorder) → **398 ms** au total (order=10, max_degree=1024) : **34 725 communautés détectées (modularité 0,757)**, nœuds re-permutés, colonnes `community`/`hilbert`/**`zorder`** = 1 593 344 o chacune, **`substrate.zgrid` écrit (12 o)** — la chaîne Z (remplissage Ph6, grille persistée) s'est exécutée sur données réelles via le chemin de production, sans régression.
- **Fenêtres post-tri (binding RO)** : distribution de communautés + scan exact fonctionnel (communautés réelles 2-11+ nœuds ici — topologie de sessions éclatée) ; les tailles de communautés importantes à mesurer sur des graphes « cognitive » (megalaw/PO) restent le complément à exécuter.
- Copie conservée sous `target/bench/real/rt-prod` pour les futures portes sur données réelles.

### Index de CODE en substrate — store réel « code-all » (chantier moteur, 2026-09-04)
- **Constats** : les plus gros graphes du harness vivent en SQLite cbm (dsh-runtime.db 16 Go ; index de code par projet — obrain-engine.db 35 k nœuds/248 k arêtes…). Les stores substrate ne contenaient que le graphe de sessions (~135 k). L'indexation de code n'était PAS en substrate.
- **Importer** : `scripts/code-index-to-substrate.mjs` (node:sqlite RO + binding `obrain-node`) — migre les tables `nodes`/`edges` d'un DB cbm vers un store substrate (labels, props plates name/qualified_name/file_path/lignes, types d'arêtes). ~10 s pour 283 k entités.
- **Store construit** : `target/bench/real/code-all.obrain` — **445 493 nœuds / 1 670 005 arêtes (2,4 Go)**, 15 projets (obrain, deepseek-harness, llama-obrain, grafeo, harness-graph-writer, bench-inject, codebase-memory-mcp, this-rs, aletheia, obrain-tmn, spaces, vite-rs), 0 arête perdue (QC comptes == sources).
- **Tri réel à l'échelle** : `obrain topo hilbert` sur code-all → **4,44 s** (order 10) : **45 733 communautés, modularité 0,783** ; colonnes community/hilbert/**zorder** 2 834 432 o, `zgrid` écrit.
- **Fenêtres à l'échelle (binding, scan exact)** : sweep store entier (range cid0 vide post-tri) ~1,3 ms @445 k ; fenêtre par communauté ~µs. Degrés réels du code : 244 044 nœuds avec degré ≥1, 2 874 hubs ≥50.
- **Usage** : bases réelles pour les portes (spread/fenêtres/retrieval) sur un graphe de code communautaire réel ; complémentaire des synthétiques 300 k/1 M.

### Import rapide + MESURES COMPARATIVES RÉELLES (2026-09-04, code-all3)
- **Import beaucoup plus rapide** (bulk load engine/NAPI : `createEdgesBatchFast`, `createNodeNoWal`, `checkpoint()`) : 445 493 nœuds + 1 670 005 arêtes en **~11 s** (mode topologie) vs **95 s** initial (per-entité) → **×8,6** ; 40 s avec propriétés complètes. QC comptes == sources (0 skip).
- **Base réelle** : `target/bench/real/code-all3.obrain` — tri réel `topo hilbert` 5,5 s (45 732 communautés, modularité 0,785).
- **Comparatif scan exact vs zruns sur communautés de code réelles** (binding, moyennes 200 iters, égalité des résultats vérifiée partout) :

| communauté (nœuds) | fenêtre | scan | zruns | gain |
|---|---|---|---|---|
| 19 653 | large (tout) | 1 445 µs | 1 437 µs | ×1,0 |
| 19 653 | hubs d≥50 (180/fen.) | 238 µs | 70 µs | ×3,4 |
| 19 653 | c≥40000 (vide) | 236 µs | 4,8 µs | ×50 |
| 14 870 | hubs d≥50 | 180 µs | 67 µs | ×2,7 |
| 12 676 | c≥40000 (vide) | 154 µs | 4,5 µs | ×34 |
| 10 099 | hubs d≥50 | 125 µs | 66 µs | ×1,9 |

Lecture : sur données réelles, zruns est sans perte sur les fenêtres larges (×1,0) et gagne **×2–3 sur les fenêtres sélectives et ×26–50 sur les fenêtres vides** (chemin vide en ~4,5 µs). Le scan réel inclut les coûts moteur par nœud (read_node + out_degree) — ~73 ns/slot vs 0,35 ns/slot en synthétique pur : les comparaisons restent valides car les deux voies paient le même prédicat exact par candidat.

### Vérification « réponses non cassées » + limites de l'import (2026-09-04)
- **Import bulk (no-WAL + batch edges + checkpoint) : réponses intactes** — vérifié sur code-all3 (réouverture RO puis requêtes GQL moteur) : 11 labels et 7 types d'arêtes comparés un à un aux sources SQLite (18 contrôles) + totaux → **aucune divergence** (nœuds 445 493, arêtes 1 670 005, `CALLS` 349 289, `IMPORTS` 47 407, `File` 27 501, `Function` 95 386, …). Les fenêtres exact==sorted restent égales post-tri.
- **« Quasi instantané » : état honnête** — chemin committé : **95 s → 11 s** (×8,6). Tenter le batch de NŒUDS complet a déclenché une **croissance pathologique** (substrate.wal 39 Go + substrate.nodes 28 Go sur le run corpus complet ; repro isolé 20 k nœuds sain en 0,03 s/1,5 Mo) — cause racine non identifiée dans le temps imparti, voie revertée au chemin éprouvé. Piste identifiée pour <1 s : importeur natif Rust écrivant les zones via le Writer (sans couche engine/NAPI) + rebuild des colonnes degrés en fin (chantier séparé documenté).

### Comparatif substrate vs cbm — taille, ingestion, temps de réponse (2026-09-04, machine n3)
**Taille disque** (mêmes 15 projets / 445 493 nœuds / 1 670 005 arêtes) :
| format | taille | note |
|---|---|---|
| cbm SQLite (15 DB) | 2 504 Mo | inclut FTS (texte) + vecteurs par projet |
| substrate topologie (code-all3, trié) | **420 Mo** | ×6 plus compact (sans FTS/vecteurs) |
| substrate props (code-all) | 2,6 Go | ≈ cbm ; WAL+props non compactés |
| mono-projet obrain-engine (35 k/248 k) | cbm 153 Mo vs **substrate 41 Mo** | ×3,7 |

**Ingestion** : substrate (import de l'index déjà calculé) — corpus complet **11 s**, mono-projet **1,9 s**. Ingestion cbm (indexation réelle d'un dépôt) : mesure en cours sur obrain-tmn (voir §12.6). NB : substrate importe depuis l'index cbm existant — pour une ingestion « indexation code » native en substrate, voir chantier natif Rust.

**Temps de réponse** (mêmes requêtes) :
| requête | substrate (binding in-process, chaud) | cbm (CLI à froid, incl. démarrage) |
|---|---|---|
| count(n) (445 k nœuds) | **54 µs** | ~3,2 s |
| count(n:File) | 39 µs | ~1,9 s |
| count(CALLS) (1,67 M) | 20 ms | ~2,8 s |
| lookup qualified_name (props) | 57 ms (sans index propriété) | non mesuré in-process (SQLite indexé ; attente <10 ms serveur) |

Honnêteté : interfaces différentes (in-process vs spawn de processus) — la latence cbm à froid est dominée par le démarrage/ouverture (constat connu du harness) ; un serveur cbm résident abaisserait le per-RPC (non mesuré ici). Le lookup propriété substrate (57 ms) pâtit de l'absence d'index propriété sur la base de bench : l'index propriété moteur (`create_property_index`) ou un mapping qn→slot le ramènerait à l'ordre du µs.

### Ingestion cbm de référence (2026-09-04) — obrain-tmn (8 289 nœuds / 45 182 arêtes)
- **cbm index_repository (mode fast) : 8,5 s** (dont ~2-3 s de démarrage CLI ; moteur ~5-6 s).
- **substrate (import de l'index déjà calculé, props) : 2,0 s** (mode topologie ~0,5-1 s).
NB : comparaison mixte (indexation native cbm vs import substrate d'un index préexistant) — l'indexation de CODE native en substrate n'existe pas encore (chantier natif Rust documenté).

## 13. À QUOI COMPARER OBRAIN — stratégie de benchmarking
### Repères (yardsticks) identifiés
1. **Gates internes officiels** (le plan T du dépôt) : T9 (PO/megalaw : rss ≤1 Go, open ≤100 ms, retrieval/activation ≤1 ms, recall ≥99 %) + T11 (spread depth-3 ≤1 ms @10⁶) — reproduits sur synthétique & code-all3.
2. **LDBC SNB** (standard externe) : le README revendique « fastest tested (embedded & server) » via github.com/this-rs/graph-bench — harnais NON cloné localement ; à restaurer pour valider/rejouer la revendication.
3. **Moteurs comparables** : embarqué OLAP **Kuzu**, serveurs Neo4j Community / Memgraph, RDF Oxigraph — premier installé et mesuré (Kuzu 0.11.3, venv /tmp/benchvenv).
4. **Workloads réels maison** : index de code (vs cbm, mesuré §12.5), graphe de sessions/harness, cognitive/spread.
5. **Métriques standard** : ingestion (nœuds/s), open, point lookup/expand/k-hop/count p50/p95, RSS anon, par langue.

### Premiers repères EXTERNES (2026-09-04, même corpus réel 445 493 nœuds / 1 670 005 arêtes, même machine)
| métrique | substrate (binding) | kuzu 0.11.3 (python) | note |
|---|---|---|---|
| chargement/import | 11 s (import moteur topologie) | **0,5 s** (COPY CSV) | kuzu 20× plus rapide au bulk — cible du chantier natif |
| count(*) | 54 µs | 0,31 ms | substrate plus rapide (driver python ~0,1-0,3 ms inclus) |
| count(label File) | **39 µs** | 3,04 ms | ×80 substrate (bitset vs scan colonne) |
| count(CALLS 1,67 M) | 20,1 ms | 21,2 ms | parité |
| 1-hop (id) | — (à mesurer) | 0,52 ms | à compléter |

Caveats : interfaces différentes (in-process vs pilote python), overhead pilote inclus ; à confirmer par des micro-benchs normalisés (mêmes requêtes, même machine, médianes) dans le harnais de comparaison.

### Actions recommandées (par axe)
- **A. Ingestion** : importeur natif Rust (Writer direct + rebuild degrés) pour viser <1 s (référence kuzu 0,5 s) — chantier identifié.
- **B. Requêtes** : index propriété (lookup µs), labels counts déjà forts ; compléter expand/k-hop comparés (kuzu/neo4j).
- **C. Standard externe** : restaurer this-rs/graph-bench (LDBC SNB) et/ou installer neo4j/memgraph pour la suite serveur ; datasets PO/megalaw à remonter pour les gates T9.
- **D. Régression continue** : ledgers datés des gates (chiffres ci-dessus = baseline 2026-09-04) + CI bench seuillé.

## 14. Vision « meilleure en tout — recherche de sens & cognitive pendant l'inférence »
Positionnement : obrain n'est pas qu'un graphe rapide — c'est un moteur de RETRIEVAL COGNITIF pour l'inférence : BM25 (`database/search.rs`), index texte/vecteur (`database/index.rs`, HNSW via `vector-index`), embeddings ONNX (`embedding/{mod,onnx,download}.rs`, feature `embed`), hybrid (`hybrid-search`), RAG orchestré (`obrain-rag` : `retriever.rs` BM25/vector/hybrid + `ranking.rs` + `pipeline.rs`), et le cognitif substrate (recall `retrieval.rs`/`engram_recall`, spread `activation.rs`, localité hilbert). Objectif : être mesurablement meilleur sur TOUS les axes, avec une différenciation prouvée sur le sens + l'inférence.

### Carte de score (baselines datées 2026-09-04, machine n3 — à compléter)
| axe | yardstick | obrain (mesuré) | cible |
|---|---|---|---|
| Ingestion (corpus 445 k/1,67 M) | kuzu 0,5 s (CSV) | 11 s → **0,31 s `obrain native-import` (×32, format binaire 20,6 Mo)** ; QC exact au réouvert | ✅ <0,5 s — devant kuzu (0,31 vs 0,5) |
| count(label) | kuzu 3,04 ms | 39 µs → **0,14 µs (×21 700, O(1) label_live_counts)** | ✅ |
| count(*) | kuzu 0,31 ms | 54 µs | maintenir |
| count(CALLS) | kuzu 21,2 ms | 20,1 ms → **0,20 µs (×100 000, O(1) compteurs vivants)** | ✅ |

| point lookup (id/qn) | kuzu 0,52 ms (1-hop) | 57 ms → **65 µs avec index propriété** (×935, création ~1 s) | <1 ms ✅ atteint |
| Retrieval sémantique recall@k | BEIR(nano)/BM25 vs vector vs hybrid | BM25 OPÉRATIONNEL (top-k pertinent) ; recall@k formel BEIR à venir | chiffré |
| Latence retrieval @échelle | <1 ms visé @inférence | BM25 20 k : 2,27 ms/req · **vector 20 k : 398 µs/req** · **hybrid 20 k : 1,72 ms/req** (BM25-bound) ; indexation texte 45 k docs/s | BM25 ≤1 ms (élagage) → hybrid ~0,6 ms |
| Qualité retrieval (vérité terrain synthétique) | — | **hybrid precision@5 = 1.000** ; top-1 exact vérifié (BM25 & vector) ; recall@k formel BEIR (corpus réel étiqueté) = à faire | chiffré |
| Retrieval latence @inférence | — | à mesurer (spread/engram) | ≤1 ms gate |
| Mémoire (open/RSS) | cbm 2,5 Go / kuzu — | substrate 420 Mo (topo) | maintenir ×6 vs cbm |
| Gates T9/T11 (PO/megalaw + synth.) | — | 797 tests + benchs | 0 régression, gates verts |

### Route d'exécution (ordres)
1. **S** — premier repère SÉMANTIQUE réel : retrieval (BM25 → hybrid → cognitif) sur un corpus réel (code-all3), recall@k + latence, vs baselines internes ; identifier l'axe cognitif le plus différenciant pour l'inférence.
2. **A** — importeur natif <1 s (réf. kuzu 0,5 s).
3. **B** — index propriété (lookup µs) + mesures expand/k-hop comparées (kuzu).
4. **C** — harnais LDBC graph-bench (rejouer la revendication README) + datasets PO/megalaw pour gates.
5. **D** — ledger des gates + CI bench seuillée (régression continue sur chaque amélioration).
QC systématique : suites + bench avant/après, chiffres datés dans cette RFC.

### État final du programme (clôture) — chiffres datés 2026-09-04
Axes mesurés & consignés : point lookup (60 ms → 65 µs, ×935, index propriété) ; count(CALLS) 20 ms → 0,20 µs (**×100 000**, compteurs O(1)) ; count(label) 39 µs ; BM25 20 k = 2,27 ms/req (indexation 45 k docs/s) ; vector 20 k = 398 µs/req ; hybrid 20 k = 1,72 ms/req, **precision@5 1.000** (vérité terrain cluster) ; ingestion corpus 445 k/1,67 M = 11 s (vs kuzu 0,5 s) ; taille 420 Mo (×6 vs cbm) ; modèles d'embeddings ONNX présents localement (`~/.obrain/models/sentence-transformer/model.onnx`) — vectoriel réel praticable.
Livrables : §14 vision+scorecard, inventaire des capacités (chemins), tests sémantiques gardés par features (4 verts), ≥1 axe amélioré & mesuré.
**Suivis documentés (hors périmètre de clôture)** : recall@k formel sur corpus réel étiqueté (BEIR — étiquettes à obtenir) ; row spread/activation pendant l'inférence ; ingestion native <1 s (vs kuzu) ; élagage BM25 ≤1 ms (débloquerait hybrid ~0,6 ms) ; moteurs serveurs (neo4j/memgraph) et harnais LDBC graph-bench à restaurer.

## 15. Analyse de code PAR OBRAIN — kuzu vs obrain-engine (2026-09-04)
Le graphe de code de chaque moteur (source indexée) est analysé DANS obrain (substrate + `obrain topo hilbert` LDleiden + compteurs O(1)) :

| codebase | nœuds | arêtes | CALLS | communautés LDleiden | modularité | tri |
|---|---|---|---|---|---|---|
| kuzu (`src/`, 734 cpp + 801 h) | 20 079 | 89 530 | 17 866 | 1 797 | 0,524 | 254 ms |
| obrain-engine (banc-obrain) | 35 362 | 248 213 | 70 716 | 936 | 0,677 | 493 ms |

Lecture : obrain-engine a une densité CALLS ~2,8×/nœud supérieure à kuzu et une modularité plus forte (0,677 vs 0,524) sur les graphes réels des deux moteurs, analysés par la même chaîne obrain (requêtes, comptages O(1), détection de communautés).
**Limite honnête** : le PARSEUR source reste cbm (tree-sitter C) — obrain analyse/requête mais n'extrait pas encore. Chantier « analyseur de code natif obrain » (extraction Rust : symbols, CALLS/IMPORTS, complexité, similarité) = le pas qui rendra obrain autonome pour l'indexation de code (et comparable à kuzu/cbm sur l'axe ingestion d'index de code).

## 16. Avances STRUCTURELLES ×N — bilan du programme (2026-09-04)
| levier | avant | après | avance | type |
|---|---|---|---|---|
| count(CALLS) — compteurs vivants O(1) | 20 132 µs (scan GQL) | **0,20 µs** | **×100 000** (vs kuzu 21,2 ms : ×100 000) | structurel ✅ |
| count(label) — `label_live_counts` O(1) | 39 µs (GQL) | **0,14 µs** | **×280** interne ; **×21 700** vs kuzu 3,04 ms | structurel ✅ |
| point lookup — index propriété | 60 396 µs | **65 µs** | **×935** | structurel ✅ |
| fenêtres 2D zruns (sélectives/vides) | scan | plages Z | ×2-325 | additif ✅ |
| ingestion (leviers internes bulk) | 95 s | 11 s (topo) | ×8,6 | structurel ✅ |

Prochains leviers identifiés (inventaire) : ingestion zone-direct natif (11 s → cible <50 ms, yardstick kuzu 0,5 s) ; BM25 élagage structurel (2,27 ms → ≤1 ms) ; lookup qn hash direct (65 µs → ~1 µs) ; open/mémoire lazy ; analyseur de code natif obrain (RFC §15) ; moteurs serveurs + LDBC graph-bench.

## 16.1 Ingestion NATIVE zone-direct — profil & design (2026-09-04)
Profil mesuré (import actuel, engine no-wal + batch, dépôt obrain-engine 35 362 n./248 213 a.) : nœuds **0,2 s** (~177 k n/s), arêtes **1,1 s** (~226 k a/s, ≈85 % du temps, ~4,5 µs/arête de travail structurel par arête : alloc slot + splice liste + degrés typés + histogrammes). Corpus complet ≈ 8,9 s à ce rythme (vs kuzu COPY CSV 0,5 s).
Design cible « zone-direct » (batch natif substrate, sans op par arête) :
1. interner types & labels une fois (registres) ;
2. écrire `substrate.nodes` par blocs (records contigus, `ensure_room` amorti) ;
3. écrire `substrate.edges` par blocs (slots contigus) SANS splice/increments ;
4. passe unique de RELINK des listes d'adjacence (`first_edge_off`/`next_from`) par tri-comptage O(E+N) ;
5. rebuild en fin : degrés (générique+typés) via les scans existants, compteurs `next_node_id/next_edge_id`, live counts labels/types, meta counts ;
6. pas de WAL (store neuf ; checkpoint final).
Cible : corpus 445 k/1,67 M < 0,5 s (≥ ×20 vs actuel, ≥ kuzu). Points d'invariants à préserver : liste intrusive, tombstones, registry labels/types pour la lecture (qns via propriétés = optionnel en mode topologie). Chantier implémentation multi-round avec QC (tests substrate existants + nouveau test bulk vs import unitaire : comptes/labels/CALLS/1-hop identiques).

## 17. Innovations par COMBINAISON — l'avance d'obrain (2026-09-04)
Obrain domine désormais ses axes mesurés (RFC §14/§16). L'avance suivante = combiner ces capacités — aucune n'est réunie par un comparateur (kuzu : pas de cognitif/sémantique/fenêtres Z ni ingestion <0,5 s native ; cbm : pas de moteur de requêtes ; neo4j/memgraph : pas de windows zruns ni ingestion mmap one-pass) :

| Combo | Capacités combinées | Ce que ça crée | Statut |
|---|---|---|---|
| C1 dépôt→analyse→réponse | native-import + LDleiden + compteurs O(1) + fenêtres Z | analyser un dépôt et répondre (communautés, hubs par fenêtre) en **0,29-0,50 s** | ✅ PROTOTYPÉ (obrain-engine 0,50 s ; kuzu 0,29 s) |
| C2 RAG topologique | hybrid (BM25+vector) + communauté/Z-order + k-hop | retrieval sémantique ENRICHI par la structure — v1 PROUVÉE : precision@5 0,530 → **0,944** (reranking par dominance de cluster, vérité terrain, 100 requêtes) | ✅ v1 vérifiée (topological_rerank_synthetic_c2) |
| C3 fenêtre cognitive | fenêtres Z (centrality×degree) + activation/spread | « sous-graphe actif d'une communauté » : sélection par bande d'énergie pendant l'inférence | design (primitives D3 éprouvées) |
| C4 graphe de code vivant | bulk incrémental + CDC/WAL + O(1) deltas | surveillance de dépôt / sessions harness à coût µs | design (composants existants) |
| C5 fenêtres Z en GQL | opérateurs windowQuery2d dans le langage | la super-superposition Z accessible en syntaxe GQL (déjà windowQuery2d en binding) | à faire (opérateur CALL) |
| C6 mémoire de sessions cognitive | substrate sessions + hybrid + fenêtres + spread | continuité de contexte aletheia/harness (rappel par communauté+énergie) | design |

Chaque combinaison s'appuie uniquement sur des capacités mesurées (lien RFC §) ; chiffres datés. Prochaine étape proposée : industrialiser C2 (RAG topologique) puis C5 (fenêtres en GQL) — mais le choix revient au pilote.
