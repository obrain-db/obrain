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
| `phase_a_write/sessions_concurrentes_everycommit/4` | 4 sessions concurrentes, `EveryCommit` | **2,134 s** ⚠ | 2,052 – 2,243 s (10 échantillons) | 78,4 ms | 3,28 → 2,31 |
| `phase_a_write/writer_unique_group_commit/4` | écrivain unique, group commit | 79,2 ms | 78,4 – 80,0 ms | 74,8 ms | 3,28 → 2,31 |
| `phase_a_write/seq_1_thread_group` | séquentiel, 1 thread, `Group` | 78,8 ms | 78,0 – 79,4 ms | 78,0 ms | 3,28 → 2,31 |

Écarts notables par rapport à e11bdd2a (255 commits plus tard) :

- `query_1hop_pattern` / `query_2hop_pattern` : ÷ 7,8 et ÷ 7,0, cohérent
  avec le filtre sans hydratation (C1, ci-dessous) qui avait déjà ramené
  `query_node_lookup_by_property` de 379 à 52,5 µs.
- ⚠ `phase_a_write/sessions_concurrentes_everycommit/4` : **2,13 s contre
  78,4 ms** (× 27). Non expliqué à ce jour : le fichier du bench a lui-même
  changé depuis e11bdd2a (cf0beb14, 4ffa0b0a) et plusieurs correctifs de
  durabilité ont été fusionnés (#123, #137, #153). Criterion ne détecte
  aucun changement par rapport à une exécution antérieure du même créneau
  (+5,6 %, p = 0,06) : la valeur ~2 s est donc stable, pas un accident de
  charge. À bissecter avant de citer ce chiffre.
- Les autres mesures sont dans le bruit (≤ ± 6 %) ou légèrement meilleures
  (`tiered_scan` rayon : − 4 %).

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
