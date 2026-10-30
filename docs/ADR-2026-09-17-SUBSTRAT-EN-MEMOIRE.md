# ADR — Substrat en mémoire : faire exister une base Obrain sans système de fichiers

**Date** : 2026-09-17. **Statut** : **APPLIQUÉ ET MESURÉ** — décision prise,
implémentée, et vérifiée par exécution. Ce document fixe le *quoi* et le
*pourquoi* ; les comptes et les commandes sont dans
`docs/WASM-HORLOGE-2026-09-17.md`.

**Nature** : architecture decision record, rattaché au chantier « T17b » que le
dépôt avait lui-même nommé en retirant l'ancienne surface de snapshot.

**Régime des affirmations.** **CONSTATÉ** = lu dans un fichier cité.
**MESURÉ** = relevé par exécution, commande reproductible citée.
**NON MESURÉ** = pas encore relevé.

---

## 1. Le problème, posé sans détour

Le binding wasm ne peut pas construire de base. Ce n'est pas une hypothèse :

**MESURÉ.** `crates/bindings/wasm/pkg/` (le build livré, consommé en
`@obrain-db/wasm`) exécuté dans Node : `init()` passe, puis `new Database()`
panique (`unsupported/time.rs`), puis — l'horloge réglée — panique à
`unsupported/os.rs:60:5: no filesystem on this platform`.

**CONSTATÉ.** La cause est structurelle : `Config::in_memory()`
(`crates/obrain-engine/src/config.rs:326`) met `path: None` ; le moteur route
vers `SubstrateStore::open_tempfile()` (`crates/obrain-engine/src/database/mod.rs:944`)
→ `SubstrateFile::open_tempfile()` (`crates/obrain-substrate/src/file.rs:938`)
→ `TempDirGuard::new` → `std::env::temp_dir()` (`file.rs:1207`).

Autrement dit : **le « mode mémoire » d'Obrain est un substrat fichier dans un
répertoire temporaire.** Tant que c'est vrai, `wasm32-unknown-unknown` — qui n'a
pas de système de fichiers — ne peut pas héberger une base.

**MESURÉ (déterminant pour la conception).** Sur `wasm32-unknown-unknown`,
`std::fs::File::open` et `std::fs::write` **retournent une erreur** ; ils ne
trappent pas. Seuls `std::env::temp_dir()`, `std::process::id()` et
`std::time::{Instant,SystemTime}::now()` trappent. Conséquence pratique : les
blocages qui subsisteront après le mode RAM seront des erreurs propres, donc
itérer est possible sans deviner.

## 2. Décision

**Insérer le mode RAM dans la couche « zone », pas ailleurs.**

1. `crates/obrain-substrate/src/file.rs` : `ZoneFile` remplace ses champs
   `file: File` + `map: Option<MmapMut>` par un choix de deux modes — fichier
   mmapé (natif, **strictement inchangé**) ou mémoire. Idem pour la méta de
   `SubstrateFile` (`meta_file`/`meta_map`, sans aucun usage externe).
2. **L'API publique de `ZoneFile` ne change pas** (`as_slice`, `as_slice_mut`,
   `grow_to`, `shrink_to`, `msync`, `fsync`, `path`, `len`, `remap_count`). Les
   ~20 modules consommateurs (blob_column, degree_column, props_zone, engram,
   simplex_*, replay, checkpoint…) n'ont rien à savoir du mode.
3. **Un registre partagé par nom** : deux poignées ouvertes sur la même zone
   voient les mêmes octets. C'est un invariant, pas un détail d'implémentation
   (voir §4).
4. Le mode RAM est **constructible sur toutes les cibles** (donc testable
   nativement), mais le moteur ne l'utilise **que sur wasm**, et seulement quand
   `config.path` est `None`. Le comportement natif reste `open_tempfile()`.

## 3. Alternatives rejetées

| alternative | pourquoi rejetée |
|---|---|
| Rendre toute la crate sans `fs` | ~250 `File::open`, 64 `File::create`, 20 `OpenOptions`, 17 `rename`, 15 `read_dir` sur **20 modules**. C'est le projet entier, pas une correction — et il casserait la persistance, qui est le métier du substrat. |
| Un système de fichiers virtuel au niveau `std` | Impossible : `File` est un type concret, non virtualisable. Les champs `file: File` devraient disparaître de toute façon. |
| Un store RAM implémentant `GraphStoreMut` | Reviendrait à réécrire le substrat (index, colonnes, degrés, zones de propriétés) pour un second backend à maintenir. Le substrat est le backend unique depuis T17. |
| `wasm32-wasip1` | Déplace le problème : un navigateur n'a pas de système de fichiers à fournir. |
| Activer le mode RAM sur natif aussi | Changement de sémantique de durabilité pour des milliers de tests existants (8 844). Refusé : le natif garde son comportement, le RAM se prouve par ses propres tests. |

## 4. Invariants à tenir (le cœur de la décision)

- **Partage par nom.** Le code existant garde des poignées DURABLES de la même
  zone en même temps (`blob_column.rs:286-287` : `idx`/`dat` ; `degree_column.rs:121` :
  `zf` ; cache de zones du `Writer`). Avec `mmap`, ces poignées partagent
  silencieusement le cache de pages. Avec des tampons possédés par poignée, elles
  divergeraient — **corruption silencieuse**. D'où le registre.
- **Aucune vue vivante pendant une croissance.** `grow_to` en mmap *remappe* :
  l'adresse change. En RAM, la réallocation déplace aussi. Le contrat est donc
  identique à celui qui existe déjà, et n'est pas assoupli.
- **Sérialisation des écritures** par les verrous du substrat, comme aujourd'hui.
- **`Send`/`Sync`** : wasm32-unknown-unknown est mono-thread ; le no-op de verrou
  wasm (`file.rs:86-114`) documente déjà ce raisonnement. Les implémentations
  devront être justifiées explicitement, comme tous les `unsafe` du fichier.

## 5. Conséquences, y compris désagréables

- Le mode RAM **ne persiste rien** : c'est sa définition. Le binding navigateur
  gagne une base éphémère, pas un stockage. Un jour, un export/import explicite
  (T17b, cf. §6) sera la seule façon de sauver un graphe.
- Les fonctionnalités qui écrivent **vraiment** sur `path()` — `checkpoint.rs:98`
  (`atomically_write_meta`), `blob_column.rs:647-648,1287,1305` — échouent en RAM.
  Le checkpoint a été TRAVAILLÉ le 2026-09-17, puis **reverté** : ses deux
  écritures de répertoire sautées, le flush aboutit, la surveillance du WAL
  déclenche un vrai checkpoint, et la ré-ancrage des index qui suit nomme ses
  fichiers temporaires avec `std::process::id()`, qui panique sur wasm32
  (« no pids on this platform ») — mesuré : suite wasm de 22/22 à 1/22. Le mode
  mémoire est donc **sans checkpoint**, et la raison n'est pas « le checkpoint
  écrit un fichier » mais sa suite. Pour y arriver : d'abord des
  `fusionner_index_*` sans `process::id` (un compteur atomique suffit).
- Les tests wasm (`crates/bindings/wasm/tests/web.rs`, 22 tests) deviennent le
  **mètre du chantier** : mesurés à `1 passed; 21 failed` avant, tous sur
  `temp_dir`. Atteindre 22/22 est la définition de « fini ».

## 5bis. Mesuré après implémentation

**MESURÉ.** `crates/obrain-substrate/src/file.rs` porte le support à deux modes
(`ZoneBacking::Mmap` | `Ram`), `RamZone` et le registre `RamDir` ;
`wal_io.rs` porte `WalMedium::{File, Memoire}` ; `store.rs` expose
`SubstrateStore::open_memory()` ; `crates/obrain-engine/src/database/mod.rs`
route vers lui **sur wasm32 seulement**.

| contrôle | avant | après |
|---|---|---|
| suite wasm du binding | 1 passé / 21 échoués (tous sur `temp_dir`) | **22 passés / 0 échoué** |
| tests natifs `tests_memoire` (mode mémoire) | — | **6 passés / 0 échoué** |
| `init()` + `new Database()` + `INSERT` + `MATCH` dans Node | trap au constructeur | **OK**, `[{p.name: "Alix", p.age: 30}]`, 1 nœud |

L'invariant de partage du §4 est prouvé par un test dédié : deux poignées
ouvertes sur la même zone se voient mutuellement écrire.

## 5ter. Ce que le mode mémoire coûte et rapporte — mesuré

**MESURÉ** le 2026-09-17 par
`crates/obrain-substrate/benches/memoire_vs_fichier.rs` (criterion, contrôle à
blanc inclus). **Deux runs, deux verdicts** — le second seul est fiable :

| bras | fichier (temp + mmap) | mémoire | écart |
|---|---|---|---|
| ouverture d'un store | 6,547 ms | **51,0 µs** | RAM **128× plus rapide** |
| création de nœuds (par 200) | 209,7 µs | **27,3 µs** | RAM **7,7× plus rapide** |
| pose de propriété | 991,4 ns | 994,1 ns | identiques (+0,27 %) |
| lecture de propriété | 100,39 ns | 101,28 ns | identiques (+0,9 %) |
| hydratation d'un nœud | 350,31 ns | 350,93 ns | identiques (+0,2 %) |

Conditions du run retenu : `MEM_BENCH_N=200`, `--sample-size 100`, charge 1,56 →
4,12. Contrôles à blanc : **+0,60 %** (ouverture) et **+0,43 %** (lecture), soit
une résolution de ~0,5 %.

**Une conclusion que j'ai dû CORRIGER.** Un premier run (`--sample-size 10`,
charge 4,09 → 6,75) annonçait la mémoire 28,6 % plus lente en lecture unitaire et
52,1 % plus lente en hydratation, et je l'avais écrit ici. C'était du **bruit** :
les contrôles à blanc du même run dérivaient déjà de **6,4 %**, contre 0,43 % au
second. Le banc a donc servi à réfuter ma propre lecture — c'est son rôle, et
c'est la discipline de `BANCS.md` qui l'a permis.

**Conséquence sur la décision :** le mode mémoire ne coûte rien en lecture et
divise l'ouverture par 128 et la création de nœuds par 7,7. Il n'y a donc **pas**
d'arbitrage « lecture contre écriture » à faire : le mode fichier reste le défaut
pour la PERSISTANCE et le multi-processus, pas pour la performance. La seule
inconnue restante est la consommation en octets.

## 6. Ce que cette décision ne règle pas

**NON MESURÉ / NON TRANCHÉ** :

- **Aucun snapshot natif substrat (T17b).** En attendant, un graphe RAM ne peut
  pas être sauvegardé. Le test de round-trip des tests wasm a été réécrit contre
  la surface réelle pour ne pas mentir, et pointe vers ce manque.
- **Le mode RAM est prouvé en Node, pas dans un navigateur** (même hôte JS, autre
  environnement).
- Le coût en **octets** du mode RAM (pas de THP, pas de `madvise`, tampons qui
  doublent à la réallocation) n'est pas mesuré. Le coût en **temps**, lui, l'est
  désormais — voir §5ter.
