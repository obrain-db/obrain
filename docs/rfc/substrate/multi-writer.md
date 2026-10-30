# Multi-writer haute performance — sans perte des lectures

**Status**: Proposition (design + filet de tests) — 2026-09-06
**Branche**: `feat/zorder` — additif strict, `SUBSTRATE_FORMAT_VERSION` inchangé
**Companion of**: `format-spec.md`, `wal-spec.md`
**Problème**: le substrate n'a qu'un writer logique par store ; « une vraie base de
données ne doit pas se limiter à son cas d'usage » — il faut N writers concurrents
sur le même graphe SANS dégrader le chemin de lecture (mmap pur, sans lock).

---

## 1. Constat — le modèle actuel (vérifié dans le code, HEAD 8f783989)

### 1.1 Le verrou de répertoire (`crates/obrain-substrate/src/file.rs`)

Un store substrate = un répertoire de zones mmap + un WAL append-only, protégé par
un `flock` OS sur `substrate.meta` (`VerrouRepertoire`, `acquerir_verrou_repertoire`) :

| Ouverture | Verrou | Écriture | Coexistence |
|---|---|---|---|
| `SubstrateFile::open` (writer exclusif) | **`LOCK_EX`** exclusif | mmap direct + WAL | seul processus |
| `open_shared_write` (GraphWriter) | **`LOCK_SH`** partagé | WAL append permis | coexiste avec lecteurs RO **mais aussi avec un 2ᵉ shared-write** |
| `open_read_only` | `LOCK_SH` partagé | refusée | N processus RO simultanés |

Fait central (doc `open_shared_write`, file.rs ~765) : le mode shared-write pose un
verrou **partagé** — « réservé au GraphWriter unique : un seul écrivain à la fois,
la sérialisation des appends WAL reste à la charge de l'appelant ». Autrement dit :
**l'unicité du writer est une convention d'appelant (`OBRAIN_WRITE=1`), pas une
garantie du verrou.** Deux processus en `open_shared_write` sur le même store ne se
refusent pas — ils corrompraient le WAL. C'est LE verrou à faire sauter proprement.

Registre intra-process : une part partagée existante refuse une demande exclusive
(`file.rs:193` — « un écrivain ne doit jamais coexister avec des lecteurs partageant
déjà le répertoire dans CE processus ») ; deux ouvertures du même chemin partagent la
même part.

### 1.2 Le chemin d'écriture (`writer.rs`, `wal_io.rs`)

- `Writer` : chaque mutation = (1) WAL d'abord (`WalWriter::append` — `Mutex<WriterInner>`
  interne, LSN monotone attribué sous le mutex), (2) mutation mmap ensuite
  (`write_node`/`write_edge`/… — WAL first, mmap second).
- `WalWriter` (wal_io.rs) : `append` sous `Mutex`, `fsync`, `truncate`, `iter_from`.
- Nœuds/arêtes : slots alloués par compteurs atomiques (`next_node_id`, `next_edge_id`).
- `splice_lock` (`Mutex<()>`) sérialise les insertions d'arêtes concurrentes sur un
  même slot (store.rs ~10547).

### 1.3 Le chemin de lecture (RO)

- `open_read_only` : mmap direct, **zéro lock sur le chemin chaud** (wal-spec §1.5 :
  « No locking on the hot read path »).
- Les lecteurs **rejouent le WAL à leur ouverture** pour voir l'état committé, puis
  lisent les zones mmap.
- Mesures datées : `count(n)` ~µs, lookup `qualified_name` ~µs, lecture pendant
  écrivain actif 0,15 ms (verrous partagés RO / writer shared-write).

### 1.4 Le parallélisme transactionnel existant (engine)

- `crates/obrain-engine/src/transaction/parallel.rs` : **ParallelExecutor** inspiré
  Block-STM (4 phases : exécution optimiste → validation des read/write sets →
  ré-exécution des conflits → commit ordonné). Documenté, testé en isolation
  (10 tests unitaires), **mais non branché** : aucun appel hors de son module.
- `transaction/manager.rs` : TransactionManager, MVCC par époques (`EpochId`),
  write-write conflict (first-writer-wins, manager.rs:177), retry borné après conflit
  (`session/mod.rs` ~3977-4031 : auto-commit rejoué 8-12×).
- `mvcc.rs` : re-exports de `obrain_common::mvcc` (`VersionChain`, `VersionInfo`).

### 1.5 Ce qui manque pour le multi-writer

1. **Sérialisation des appends WAL** : intra-process OK (Mutex), inter-process = rien.
2. **Exécution parallèle** : ParallelExecutor existe mais n'est pas câblé sur
   `session.execute` — tout commit passe par la voie séquentielle.
3. **Le point de sérialisation** doit être le WAL (append court, group commit), pas
   l'exécution ni les zones.
4. **La cohérence des lecteurs** : aujourd'hui « rejeu à l'ouverture » seulement —
   pour le multi-writer il faut un rattrapage de queue (mécanique `cdc_tail`/`cold_zone_delta`
   déjà présente, à réutiliser comme voie de cohérence, pas seulement pour le CDC).

## 2. Principes directeurs (ne pas violer)

1. **Le chemin de lecture reste mmap pur et sans lock** — aucune coordination
   d'écriture ne doit s'intercaler dans les lectures (`count`, lookup, fenêtres,
   scan). Les lecteurs ne paient JAMAIS la coordination des writers.
2. **Sûreté mmap** : un seul mutateur physique par zone (le mergeur/checkpointer).
   Les writers n'écrivent pas les zones directement.
3. **Zéro perte** : tout commit est dans le WAL avant d'être visible/durable ;
   replay idempotent ; `SUBSTRATE_FORMAT_VERSION` inchangé (additif strict).
4. **Le WAL est le point de sérialisation** : les writers exécutent en parallèle,
   committent par append groupé, et la matérialisation des zones est découplée.
5. **Phasage réversible** : chaque phase = tests + bench daté + ledger + commit
   unique ; pas d'amélioration sans gain mesuré (règle du dépôt).

## 3. Design cible — trois rôles découplés

| Rôle | Aujourd'hui | Cible multi-writer |
|---|---|---|
| **Exécuter** les transactions | voie séquentielle | N exécuteurs parallèles (ParallelExecutor branché sur les sessions, validation des read/write sets) |
| **Sérialiser** les commits | « à la charge de l'appelant » (1 writer) | **append WAL groupé** sous un verrou d'append court (inter-process : `O_APPEND` atomique ou segments par writer + LSN global) |
| **Matérialiser** les zones | le writer mute les zones | un **mergeur/checkpointer unique** applique WAL → zones (mécanique `merge_wal_post_flush` existante, 3 ms/Mo) |

Les lecteurs : zones mmap matérialisées jusqu'à leur époque + **rattrapage de queue
WAL depuis leur époque** (réutiliser l'index de queue `cdc_tail` comme voie de
cohérence générique, pas seulement pour search_delta).

### 3.1 Phase A — Multi-writer logique in-process (embarqué)

But : N sessions/connexions **du même process** écrivent en parallèle, commits
groupés, lectures inchangées. Ne touche pas au verrou OS (pas de multi-process).

> ⚠️ **CONSTAT MESURÉ 2026-09-06 — le design initial de cette section est
> PARTIELLEMENT INVALIDÉ par la mesure** (bench `phase_a_write_goulot`, store
> réel, 2000-4000 CREATE) :
>
> | Bras | 1 thread | 4 threads | 8 threads | Lecture |
> |---|---|---|---|---|
> | N sessions auto-commit (chemin réel) | 43-67 ms | 21-37 ms | 24-47 ms | **scale ~2× puis régresse** |
> | group commit WAL (`Group` + 1 fsync) | = EveryCommit | **0 % gain** | — | le fsync n'est PAS le goulot (cache OS) |
> | crud direct (sans manager) | 5,6 ms | 8,6 ms | 8,8 ms | substrate encaisse ~580-700k/s |
> | `record_write` seul (lock manager) | 11,0 M/s | 4,8 M/s | **2,0 M/s** | **le RwLock global DÉGRADE avec les threads (−82 %)** |
> | CREATE sans prop vs 1 prop | 6,5 µs vs **15,2 µs** | — | — | **les propriétés = le coût dominant par write** |
>
> **Conclusions** :
> 1. Le multi-writer logique sémantique EXISTE DÉJÀ (conflits write-write via le
>    `TransactionManager` partagé + retry auto-commit à jitter) — le speedup 2×
>    à 4 threads le prouve.
> 2. Le group commit WAL (levier supposé n°2) = **0 % de gain mesuré** — à
>    abandonner comme levier Phase A.
> 3. `ParallelExecutor` (Block-STM, mort aujourd'hui) ne peut rien orchestrer de
>    plus que les threads ne font déjà tant que `record_write` prend le
>    `RwLock` global du manager à CHAQUE write — c'est le vrai plafond de scale.
> 4. `record_read` n'est appelé NULLE PART → la validation est write-write
>    seulement ; un Block-STM sans read sets réels serait cosmétique.
> 5. Le coût dominant par opération est le chemin de PROPRIÉTÉS
>    (`set_node_property` : internement clé + WAL + blob-check + PropsZone v2
>    + DashMap), ORTHOGONAL au multi-writer — 1 prop = +7-9 µs même en crud.

Le design Phase A est donc RE-SCOPÉ sur les coûts mesurés du chemin réel (voir
§3.1bis), pas sur l'orchestration par lot ni sur le lock du manager.

Porte A (re-scopée, chemin réel session GQL CREATE 1 prop) : N threads écrits
parallèles — throughput @8 ≥ throughput @4 (aujourd'hui : 73k @4 → 66k @8, la
régression 4→8 doit disparaître) SANS régresser @1 (26,6 µs/op inchangé) ;
lectures concurrentes RO non régressées (count/lookup/fenêtres, RSS) ;
0 corruption après rejeu ; suites engine/substrate vertes.

### 3.1bis Phase A re-scopée — réduire la contention du commit (levier mesuré)

Le goulot n'est PAS le WAL/fsync (0 % gain group commit) ni l'orchestration
(le multi-writer logique existe déjà) : c'est le **`RwLock` global du
`TransactionManager` pris par `record_write` à CHAQUE write** (`manager.rs:183`
— `transactions.write()` acquis avant même le test `active_count`, puis scan
de toutes les transactions actives quand >1). Mesure : 11 M/s @1 thread →
**2 M/s @8 threads (−82 %)** sur entités disjointes (conflit impossible).

Pourquoi le scan est nécessaire (exactitude) : deux transactions ACTIVES
concurrentes écrivant la même entité E créeraient des PENDING imbriqués
impossibles à rollback par transaction — le commit() seul ne les détecte pas
(il ne scanne que les transactions déjà COMMITTÉES). Le first-writer-wins
précoce n'est pas une optimisation : c'est la garde d'intégrité MVCC.

> ⚠️ **CONSTAT D'IMPLÉMENTATION 2026-09-06 (levier A-2 IMPLÉMENTÉ puis
> RETIRÉ)** : l'ownership map `write_owners: DashMap<EntityId, TransactionId>`
> (CAS atomique par entité + retrait commit/abort + libération write_set sous
> le lock) a été codée et testée — 24 tests manager + 683 engine + 7
> invariants VERTS. **La mesure l'a fait retirer** :
>
> | Bras (200k record_write, entités disjointes) | @1 | @8 |
> |---|---|---|
> | AVANT (scan sous lock global) | 11,0 M/s | 2,0 M/s |
> | APRÈS (CAS DashMap + write_set sous lock) | **4,6 M/s** | 3,0 M/s |
>
> Le CAS DashMap ajoute un coût fixe par write (~120 ns) qui RÉGRESSE le
> mono-writer (11 → 4,6 M/s, −58 %) pour ne gagner que +50 % à 8 threads —
> et le write_set reste sous le lock global (le vrai goulot n'est PAS retiré).
>
> **Décision : le lock du `TransactionManager` n'est PAS le goulot du chemin
> réel.** Décomposition du chemin réel (session GQL CREATE 1 prop, ~15 µs @8) :
> `record_write` ≈ 0,3 µs (2 %). Le chemin PROPS ne scale pas non plus
> (crud 1 prop : 9,0 µs @1 → 8,2 µs @8) — le substrate sérialise. La RFC
> re-scope donc Phase A sur les DEUX vrais coûts mesurés (ci-dessous), pas sur
> le manager.

> ⚠️ **CONSTAT D'IMPLÉMENTATION 2026-09-06 (SUITE — le vrai goulot est le
> write SYSCALL WAL, pas le fsync)** : `SyncMode::Group` existant ne groupe
> QUE les fsync — chaque append fait TOUJOURS un `write_all` syscall sous le
> `Mutex<WriterInner>` (`wal_io.rs:128-131`). Mesures (WalWriter réel,
> 200k appends) :
>
> | Mesure | @1 | @4 | @8 |
> |---|---|---|---|
> | append WAL (write syscall/record) | 946k/s (1,06 µs) | 538k/s | **504k/s (1,98 µs) −47 %** |
> | EveryCommit vs Never (crud create) | 1,64 vs 1,53 µs | — | ~identique |
> | **write_all GROUPÉ (1 syscall/64 rec)** | — | — | **25,5 M/s (×22)** |
>
> → Le goulot multi-thread du chemin d'écriture est le **write syscall
> sérialisé sous Mutex** (le fsync est quasi gratuit, cache OS). Le plafond du
> chemin réel @8 (~15 µs CREATE 3 props) ≈ 7-8 appends WAL sérialisés à ~2 µs
> chacun — tout converge vers l'append. **Le group commit d'APPENDS (bufferiser
> les records, un `write_all` pour N records + un seul flush/fsync de groupe)
> est le levier mesuré : ×22 potentiel au proto**, SANS toucher au
> TransactionManager ni aux lectures (aucun lock ajouté au chemin RO).

> ⚠️ **POINT D'EXACTITUDE — RÉSOLU LE 2026-10-06** : l'ordre physique du WAL
> n'est plus un ordre distinct de l'ordre LSN. Le LSN est dérivé de la position
> (`lsn = ancre + position`), donc l'ordre physique **est** l'ordre LSN par
> construction (addendum en fin de document). Un buffer FIFO de group commit
> (`WalWriter::append_batch`) préserve l'ordre du lot → le rejeu reste exact.

### 3.1ter Phase A — levier A-3 : group commit d'APPENDS WAL — INVALIDÉ par l'exactitude

Le goulot multi-thread est le **write syscall** de l'append WAL, pas le fsync
(EveryCommit ≈ Never : le fsync est quasi gratuit, cache OS) et pas le lock du
TransactionManager (2 % du chemin réel). Mesures WalWriter réel (200k appends) :
append = **946k/s @1 → 504k/s @8 (−47 %)**, 1 write syscall/record sous le
`Mutex<WriterInner>` ; décomposition : Mutex+memcpy = **43 M/s**, append réel =
**985k/s** → le syscall est ~1 µs/record ; `write_all` groupé par 64 = **25,5 M/s
(×22)** au proto.

> ⚠️ **MAIS le group commit d'APPENDS est INVALIDÉ par une contrainte
> structurelle** (analysée le 2026-09-06) : dans le substrate, chaque write
> (`write_node`, `update_node`, edges, props…) fait **append WAL PUIS mutation
> mmap immédiate**. L'ordre WAL→mmap n'est pas un détail : si le process crashe
> PENDANT la mutation mmap (copy partiel d'un slot), le rejeu relit le WAL et
> réapplique le record pour réparer le slot. Si l'append était bufferisé en
> mémoire utilisateur (write syscall retardé), un crash pendant la mutation mmap
> laisserait un slot corrompu SANS trace WAL → corruption PERSISTANTE,
> irréparable au rejeu.
>
> Bufferiser les appends exigerait de reporter AUSSI les mutations mmap (les
> appliquer en lot APRÈS le write WAL groupé) — mais les mutations mmap sont la
> visibilité immédiate des autres sessions (lecture directe du mmap partagé) ;
> les retarder change la sémantique de lecture. **Le write syscall par record
> est un coût STRUCTUREL de l'architecture « écriture directe mmap + WAL-first
> pour réparation ».** Le levier A-3 est donc abandonné comme chantier Phase A.

**Conséquence (re-scope honnête)** : dans l'architecture actuelle, le
multi-writer mono-store est plafonné par le write syscall WAL sérialisé — et ce
plafond est structurel, pas un verrou à retirer. Le multi-writer qui scale
VRAIMENT passe par le **partitionnement (Phase B1 : plusieurs stores = plusieurs
WAL = plusieurs fichiers = parallélisme réel)** — le modèle `named_graphs` →
`Arc<SubstrateStore>` existe déjà, et le cas d'usage (import code bulk pendant
que le runtime écrit) est un store par writer. Voir §3.2.

Levier 2 (MESURÉ, ORTHOGONAL, valable aussi mono-writer) : le coût PROPRIÉTÉS
par write. `set_node_property` ≈ **+7-9 µs/prop même en crud** (internement clé
journalisé + effacement ombre blob + PropsZone v2 + DashMap), ne scale pas
(9,0 → 8,2 µs @8) et domine le CREATE GQL réel (15-26 µs avec 1 prop vs 6,5 µs
sans). Chantier distinct, premier poste de dépense du chemin d'écriture réel.

Levier 2bis : la machinerie par statement du chemin session (6,5 µs CREATE GQL
sans prop vs 1,3 µs crud = ~5 µs de parse→plan→executor→auto-commit par
statement) — à décomposer si le Levier 2 ne suffit pas.

Levier 3 (ultérieur) : `record_read` câblé + vrai SSI si Serializable doit
devenir autre chose qu'un write-write — hors périmètre Phase A (aucun appelant
ne remplit read_set aujourd'hui ; le brancher SANS usage = coût pur).

### 3.1quater Phase A — verdict de la mesure (2026-09-06)

Le multi-writer logique (conflits + retry) EXISTE et scale ~2× à 4 threads sur
le chemin réel. Le plafond au-delà n'est ni le `TransactionManager` (record_write
= 2 % du chemin réel), ni le fsync (gratuit), ni le coût propriétés seul : c'est
le **write syscall WAL par record** (~1 µs, 1 syscall/mutation sous Mutex) — et
ce coût est **STRUCTUREL** : l'ordre WAL→mmap impose le syscall avant chaque
mutation mmap, donc le group commit d'appends (A-3, §3.1ter) est INVALIDÉ par
l'exactitude (fenêtre de corruption irréparable).

**Ce que Phase A a établi (livrable)** :
1. Le multi-writer logique SÉMANTIQUE est prouvé (I5-I10 + suites) et scale
   ~2× à 4 threads — c'est déjà le comportement multi-writer du moteur.
2. Les leviers supposés sont TOUS mesurés et rejetés : group commit fsync
   (0 %), ParallelExecutor (sans read sets ni goulot d'orchestration),
   ownership map manager (−58 % mono-writer), group commit d'appends
   (invalide par l'ordre WAL→mmap).
3. Le vrai coût restant du chemin d'écriture est le coût PROPRIÉTÉS
   (+7-9 µs/prop, Levier 2) — orthogonal au multi-writer, chantier distinct.
4. Pour scale multi-writer au-delà de ~2× mono-store : **Phase B1
   (partitionnement par stores)** = plusieurs WAL = parallélisme réel — le seul
   chemin qui ne heurte pas le syscall sérialisé. C'est la recommandation de
   sortie de Phase A.

> **Piste zerocopy — mesurée le 2026-09-06, IMPLÉMENTÉE puis mesurée en réel** :
>
> | Micro-mesure | Coût | Verdict |
> |---|---|---|
> | encode WAL (bincode serde + 2 `Vec` allocs/record) | 0,102 µs | le coût est **bincode serde (~0,09 µs)**, PAS les allocs (0,01 µs) |
> | encode_into (buffer réutilisé, header en place) | 0,092 µs | les allocations étaient quasi gratuites — ×21 initial = artefact (comparé à un encode SANS bincode) |
> | write syscall WAL | ~0,94 µs | **90 % de l'append** — structurel, zerocopy n'y peut rien |
> | copie page props 4 Ko | 0,032 µs | négligeable (CRC32 0,29 µs domine) |
> | set prop page chaude vs neuve | 4,1 vs 8,3 µs | Δ ~4 µs = page neuve + WAL update, pas des copies |
>
> **Implémentation livrée** : `WalRecord::encode_into` (buffer thread-local
> réutilisé, header écrit EN PLACE puis payload en UNE passe — plus de copie
> payload 2×) + test bit-identique (`encode_into_bit_identical_to_encode`,
> 6 variantes de payload). Suite substrate **808/808 verts**.
>
> **Mesure A/B réelle (append WAL, 200-400k records, 5 runs)** :
> mono-thread 946k/s → **1,03-1,06 M/s (+10 %, stable)** ; multi-thread inchangé
> (~520k/s @8, le syscall sous Mutex domine toujours). Sur le chemin réel CREATE
> complet (~15-26 µs), l'encode n'est qu'une fraction → gain global ~1-3 %.
>
> **Verdict final zerocopy** : utile comme micro-gain mono-thread sûr (+10 % sur
> l'append isolé, format disque inchangé), mais il ne change PAS le plafond
> multi-thread — le write syscall par record (~0,94 µs) est 90 % du coût et il
> est structurel (ordre WAL→mmap, RFC §3.1ter). **45 M/s par thread en écriture
> durable n'est pas atteignable** : le syscall borne chaque thread à ~1 M/s de
> writes journalisés. Le scale multi-writer passe par B1 (partitionnement),
> pas par zerocopy.

### 3.2 Phase B1 — Multi-writer par partitionnement (stores dédiés)

But : writers physiques multiples sur des **shards** = stores substrate distincts
(le modèle `named_graphs` → `Arc<SubstrateStore>` existe déjà), routage par
graphe/préfixe `qualified_name`.

- 1 writer + N RO par shard (le modèle actuel, éprouvé) ; catalogue global au-dessus.
- Cas d'usage direct : **indexation de code (bulk 1,67 M) pendant que le runtime
  écrit ses sessions** — deux writers sur deux stores, même moteur, zéro contention.

> ✅ **B1 MESURÉ le 2026-09-06 — le partitionnement scale déjà** : chaque graphe
> nommé = `SubstrateStore` séparé (WAL + PropsZone + locks propres,
> `create_named_graph` → `store::create(chemin)`). Workload CREATE 1 prop,
> N threads × N graphes :
>
> | Voie | @1 | @2 | @4 | @8 |
> |---|---|---|---|---|
> | UN graphe partagé (GQL) | 55k/s | 84k/s | 118k/s | **118k/s (plafond)** |
> | N graphes (GQL, B1) | 53k/s | 92k/s | 121k/s | **199k/s** |
> | N graphes (crud direct, SANS manager) | 116k/s | 220k/s | 415k/s | **662k/s (×5,7, quasi linéaire)** |
>
> Le crud direct sur stores disjoints scale ~linéaire (×5,7 à 8) : les WAL/locks
> séparés ne se marchent pas dessus. **Le résidu du chemin GQL vient du
> `TransactionManager` partagé** (toutes sessions = même `Arc<TransactionManager>`,
> begin/commit/write_set par statement passent par son lock global même sur des
> graphes disjoints) + la machinerie par statement (parse→plan→executor).
> Traiter le manager (voir §3.1quater Levier 2bis) est ce qui rapproche le GQL
> du scale crud.

> ⚠️ **B1 — BUG D'ARCHITECTURE IDENTIFIÉ (faux conflits inter-graphes)** : les
> write_sets du `TransactionManager` ne sont PAS scopés par graphe.
> `EntityId::Node(NodeId)` est global, or chaque `SubstrateStore` alloue ses
> NodeId à partir de 1 indépendamment → deux transactions sur des graphes
> DIFFÉRENTS qui écrivent chacune `NodeId(1), NodeId(2), …` sont vues en
> conflit par le manager. Mesuré (4 threads × 1 tx longue × 1000 CREATE sur
> 4 graphes distincts) : **2999 faux conflits sur 4000 (75 % d'échec)** —
> exactement le scénario bulk B1 (import code + runtime). Le GQL B1 à 8
> threads (199k/s vs 662k/s crud) est plafonné par ces conflits + les retries.
>
> **Fix B1 IMPLÉMENTÉ (`b8933a48`)** : scoper les write_sets par graphe.
> `GraphStoreMut::graph_scope()` (défaut 0 = mono-graphe inchangé),
> SubstrateStore = hash FNV-1a du chemin, write/read sets = `(scope, entité)`,
> tracker créé par requête avec le scope du store. Résultat : faux conflits
> inter-graphes **0** (avant 75 % d'échec), vrais conflits intra-graphe
> préservés (I7 vert), nouvel invariant I11. Suites vertes (683/808/870/8).

> ⚠️ **B1 — NOUVEAU GOU LOT MESURÉ (round 2026-09-06, suite du fix scope)** :
> après élimination des faux conflits, le chemin GQL de MUTATION scale encore
> moins que le SELECT et le crud direct. Mesures à 8 threads × 8 graphes
> (stores disjoints) :
>
> | Chemin | @8 | Scale @1→@8 | Verdict |
> |---|---|---|---|
> | SELECT pur (sans tx) | 1,23 M/s | **×4,3** | la machinerie parse/plan/exec scale |
> | crud direct (sans manager) | 2,88 M/s | ×5,7 | le substrate multi-graphe scale |
> | GQL CREATE (auto-commit) | 390k/s | ×2,8 | plafonné par la transaction par statement |
> | begin+commit vides (manager) | 207k/s | **×0,16 (régresse)** | lock global + epoch |
> | begin+write+commit | 93k/s | ×0,09 | record_write double le coût @8 |
>
> → **Le goulot résiduel est le `TransactionManager` GLOBAL** : chaque statement
> de mutation auto-commit paie begin + record_write + commit sous le lock
> global + l'epoch `fetch_add(SeqCst)` + le scan des write_sets actifs — même
> pour des transactions sur des graphes disjoints qui ne peuvent PAS
> conflictuer (le scope les sépare déjà). Le substrate a déjà son epoch PAR
> store (`epoque_courante`) ; le manager global duplique une epoch unique pour
> tous les graphes → sérialise tous les commits.
>
> **Design B1-2 (prochain chantier)** : découpler le TransactionManager par
> graphe — chaque store (racine + nommés) porte SON manager (son lock, son
> epoch, ses committed_epochs), résolu par la session selon le graphe actif.
> Transactions multi-graphes (`touched_graphs`) : l'epoch reste cohérente car
> chaque store a déjà la sienne ; la validation croisée se fait par scope.
> Porte : begin+commit disjoints @8 ≥ @1 (au lieu de ×0,16) ; SELECT/lectures
> non régressées ; suites vertes.

> ✅ **B1-2 — CAUSE PRÉCISE MESURÉE + PROTO VALIDANT** : le scan de conflit
> n'est PAS le coût (même scope 0,64 µs = scopes disjoints 0,67 µs @8) — c'est
> le **lock `transactions.write()` GLOBAL lui-même**, pris par `record_write`
> pour insérer dans SON propre write_set. Proto : write_set PAR TRANSACTION
> (chaque tx a SON `Mutex<HashSet>`, insertion sans lock global) → **19,6 M/s
> @1 → 134 M/s @8 (×6,9, quasi linéaire)** vs lock global actuel 8,6 M/s @1 →
> 1,7 M/s @8 (×0,2, régresse).
>
> **Design B1-2 (implémentation)** : sortir les write_sets/read_sets de la map
> `transactions` (sous lock global) vers des slots par transaction
> (`DashMap<TxId, Arc<Mutex<HashSet>>>`). `record_write(scope, tx, E)` :
> 1) vérifier tx Active (read lock court, partagé — les writers ne se
> bloquent pas) ; 2) scan first-writer-wins sous le read lock (partagé, lit
> les write_sets des autres tx actives du MÊME scope) ; 3) insérer dans SON
> slot (Mutex par tx, pas global). L'atomicité scan+insertion est préservée
> par le read lock du scan — les writes concurrents sur la même entité se
> voient via les slots. commit/abort/gc : adapter la lecture/écriture des
> slots. Sémantique inchangée (I7/I11 verts).

> ❌ **B1-2 — IMPLÉMENTÉ PUIS RETIRÉ (2026-09-06, round 2)** : le refactor
> record_write → slots par tx + ownership CAS scopé a été CODÉ, TOUTES les
> suites passaient (683/61/9/27), mais la mesure l'a tué :
>
> | Chemin | Avant (lock global) | Après (slots+CAS) | Verdict |
> |---|---|---|---|
> | record_write mono @1 | 0,12 µs | **0,32 µs** | RÉGRESSION ×2,7 |
> | record_write @8 (scopes disjoints) | 1,7 M/s (×0,2) | 8,7 M/s (×2,2) | scale gagné MAIS… |
> | **GQL B1 réel @8 (CREATE 1 prop)** | **195k/s** | **169k/s** | aucun gain, léger coût |
>
> → **record_write n'est PAS le goulot du chemin réel**. Sur un CREATE GQL
> auto-commit, begin+commit par statement dominent (22 µs @1 dont ~0,1 µs de
> record_write). Le proto ×6,9 (insertion pure sans scan ni CAS ni check
> Active) ne se traduit pas : la détection réelle (CAS DashMap par entité +
> check Active par write) coûte plus que ce qu'elle retire, et le GQL est de
> toute façon plafonné par le commit. Revert complet (manager + Cargo).
>
> ✅ **B1-2 — LE VRAI GOULOT EST LE COMMIT/BEGIN PAR STATEMENT** : commit vide
> mesuré : **0,88 µs @1 → 5,02 µs @8 (×0,18, régresse)**. Structurellement :
> (a) `current_epoch.fetch_add(SeqCst)` — UN compteur GLOBAL pour TOUS les
> graphes (le substrate a déjà `epoque_courante` PAR store) ; (b)
> `transactions.write()` + `committed_epochs.write()` — locks globaux pris à
> CHAQUE commit, même pour des tx de graphes disjoints qui ne peuvent pas
> conflictuer. Chaque CREATE GQL auto-commit = begin + commit = 2 passages
> sur ces sérialisations globales.
>
> **Design B1-2 (v2)** : epoch PAR SCOPE. Le manager garde `current_epoch`
> pour la sémantique mono-graphe (scope 0, inchangée), mais les commits
> d'une tx dont le write_set est confiné à un scope ≠ 0 utilisent un epoch
> par scope (DashMap<scope, AtomicU64>) : deux graphes disjoints n'incrémentent
> JAMAIS le même compteur → zéro sérialisation croisée. La validation au
> commit ne compare que les tx du MÊME scope (les write_sets sont déjà
> scopés). begin : `start_epoch` = epoch de SON scope (résolu au premier
> write ou au premier USE). Porte : commit disjoint @8 ≈ @1 ; GQL B1 @8 >
> 400k/s ; mono-graphe inchangé (scope 0). C'est le chantier qui débloque le
> scale GQL B1.

> ⚠️ **B1-2 v2 — DEUX GOULOTS DISTINCTS, LA MESURE COMPLÈTE (2026-09-06,
> round 2)** : le commit a été décomposé et révèle que la validation contre
> `committed_epochs` est le poste dominant, PAS le lock en soi :
>
> | Variante commit @1 (20k tx) | Coût | Cause |
> |---|---|---|
> | SANS gc (committed grossit) | **10,19 µs** | validation O(N) : itère TOUT committed_epochs à chaque commit |
> | AVEC gc/100 | **0,15 µs** | committed_epochs reste borné |
> | gc/1000 (bench précédent) | 0,88 µs | compromis |
>
> → Le chemin réel : le GC de session est `gc_interval = 100` par défaut
> (config.rs:317), mais le chevauchement multi-session fait grossir
> `committed_epochs` GLOBAL entre deux GC (le GC ne retire que les commits
> antérieurs au min des start_epoch ACTIFS — sous N sessions qui se
> chevauchent, presque rien n'est retirable). La validation au commit d'une
> session scanne donc les commits de TOUTES les sessions, pour des tx de
> graphes disjoints qui ne peuvent pas conflictuer.
>
> **Deux goulots orthogonaux** (le refactor (a) seul était insuffisant —
> mesuré sur auto-commit dominé par (b)) :
> - **(a) record_write** : lock `transactions.write()` global par write →
>   domine les TX LONGUES (le cas B1 bulk : 1 begin + N writes + 1 commit).
>   Proto slots par tx : ×6,9 @8. Implémentation complète testée puis
>   RETIRÉE (mono 0,12 → 0,32 µs, CAS+check Active par write trop chers) —
>   à refaire avec fast-path mono propre.
> - **(b) commit** : validation O(committed_epochs GLOBAL) par statement →
>   domine l'AUTO-COMMIT. Fix = validation SCOPÉE : ne scanner que les tx
>   committées du MÊME scope (index scope → committed, ou committed_epochs
>   par scope). Indépendant de (a).
>
> Le scale GQL B1 réel (44k @1 → ~174k @8, ×3,9 puis plafond) est la
> CONJONCTION des deux : chaque CREATE auto-commit paie (b) à chaque
> statement et (a) par write.
>
> ❌ **Fix (b) — validation scopée implémentée puis RETIRÉE (round 2, suite)** :
> l'index `committed_by_scope` (scope → {TxId:epoch}) a été codé (validation
> write-write + SSI ne scannant que les scopes de notre write_set, GC et
> mark_committed maintenus). Suites vertes (61 manager, 9 invariants) MAIS la
> mesure a tué l'approche :
>
> | Variante (20k tx, 1 write scopé par tx) | @1 | @4 | @8 |
> |---|---|---|---|
> | index scopé AVEC gc/100 | 1,43 µs | 1,86 µs | **2,91 µs** (×0,5) |
> | index scopé SANS gc | **164 µs** | 61 µs | 55 µs |
>
> → Deux causes : (1) SANS gc, `transactions` + `committed_by_scope`
> grossissent SANS borne et le commit clone write_set + scoped_committed à
> chaque commit → explosion. (2) AVEC gc/100, le GC lui-même (sous lock
> transactions.write global, balayage complet pour min_active_start +
> retrait des index) devient le point chaud sous N threads — il tourne 100×
> plus souvent que le commit qu'il sert.
>
> **Conclusion du round 2** : le TransactionManager à epoch GLOBALE ne scale
> pas par de simples patchs locaux (record_write, validation scopée) — le
> GC global, les clones de write_set par commit et l'epoch SeqCst restent des
> sérialisations. Le plafond GQL B1 (~×3,9 @8, puis 174k/s) est cohérent
> avec un chemin d'écriture qui a ~6-8 sérialisations globales par statement.
>
> **Design B1-2 v3 (prochain chantier, refactor du cœur)** : epoch PAR SCOPE
> COMPLET, pas un patch :
> - `committed_by_scope` comme SEULE source (plus de committed_epochs global
>   pour les scopes ≠ 0 ; scope 0 = mono-graphe garde la sémantique actuelle).
> - GC PAR SCOPE : chaque scope nettoie SES commits (min_active_start par
>   scope), sans balayage global.
> - begin/commit : l'epoch et les locks ne touchent QUE le scope de la tx
>   (résolu au begin via le graphe actif de la session — USE GRAPH pose le
>   scope avant begin dans le chemin réel).
> - Pas de clone de write_set au commit : la validation lit les write_sets
>   des tx du scope sous leurs slots (voir (a)).
> Porte : tx disjointes @8 ≥ @1 (zéro sérialisation croisée) ; mono-graphe
> (scope 0) bit-identique. Risque : moyen-haut (cœur MVCC) → RFC dédiée +
> I7/I8/I11 verts à chaque étape.

> ✅ **B1-2 v3 — PROTO VALIDANT (round 3, 2026-09-06)** : begin+commit
> complets (begin : epoch+insert map ; commit : validation vide + mark
> committed + remove) sur des structures INDÉPENDANTES par thread (= le cas
> B1 : chaque graphe a SON manager) :
>
> | Structure | @1 | @8 | Scale |
> |---|---|---|---|
> | maps/locks PAR SCOPE (proto) | 167 ns/paire | **25 ns/paire** | **×6,6** |
> | manager GLOBAL actuel (mesuré) | ~0,88 µs | ~5 µs | ×0,16 |
>
> → **~200× d'écart à 8 threads**. Le chemin GQL B1 réel (begin+commit =
> 1,13 µs/statement @8, 31 % du CREATE 1 prop complet à 3,62 µs) tomberait à
> ~0,1 µs. Preuve que la sérialisation croisée du manager GLOBAL (et non
> l'epoch SeqCst : 8,6 ns vs 1,2 ns par scope — négligeable) est LE plafond.
>
> **Design B1-2 v3 — implémentation retenue** : manager PAR GRAPHE ≠ patch
> interne. Chaque SubstrateStore (racine + nommés) porte SON
> `Arc<TransactionManager>` (résolu par la session comme `resolve_store` —
> begin/commit/write tracker utilisent le manager du graphe actif). Le
> TransactionManager reste inchangé (lock global PAR manager = par graphe) →
> mono-graphe bit-identique, risque contenu au routage session. Tx
> multi-graphes (`USE GRAPH` en cours de tx) : le manager du graphe du
> premier write porte la tx ; les stores supplémentaires sync epoch comme
> aujourd'hui (epoch par manager = par graphe, le substrate a déjà
> `epoque_courante` par store). Étapes : 1) ObrainDB crée un manager par
> store enfant (`create_named_graph`) + le racine ; 2) la session expose
> `manager_du_graphe_actif()` et l'utilise pour begin/commit/abort/GC ;
> 3) I7/I11 verts (conflits intra-graphe inchangés) ; 4) bench GQL B1 @8.

> ⚠️ **B1-2 v3 — DONNÉES DU PLAFOND MATÉRIEL (round 4, 2026-09-06)** : la
> machine de dev a 8 cœurs physiques (AMD Ryzen 7 8845HS, 16 threads SMT) —
> le scale maximal mesurable y est **×4,6-4,7** (SELECT pur 8 threads :
> 325k mono → 1,53M @8), pas ×8 (turbo mono ~5 GHz vs ~3,5-4 GHz sur 8
> cœurs + mémoire partagée). Mesures à 8 threads × 8 graphes (benches longs,
> alternés pour neutraliser la dérive thermique) :
>
> | Chemin | @8 | Scale vs mono | % du plafond ×4,7 |
> |---|---|---|---|
> | SELECT pur | 1,53 M/s | ×4,7 | 100 % |
> | substrate nu create+prop (8 stores) | 480k/s | ×4,1 | 87 % |
> | GQL CREATE sans prop | 398k/s | ×2,9 | 61 % |
> | GQL CREATE 1 prop | 261-306k/s | ×3,3-4,0 | 70-85 % |
>
> → Le substrate multi-graphe scale déjà à 87 % du plafond physique ; le
> GQL B1 à 70-85 %. La marge restante du GQL ≈ la couche transactionnelle
> (begin+commit = 1,13 µs @8 mesuré, record_write 0,67 µs @8) — le design
> v3 (manager par graphe) la récupérerait, plafonné à ~×4,7 matériel.
>
> **Levier 2 — mesures du round 4 (toutes vérifiées, chantiers écartés)** :
> la 1re prop (page neuve) = 7,5 µs vs prop page chaude = 2,35 µs vs
> l'écrasement même clé = 2,35 µs. Instrumentation fine (chronos par étape) :
> allouer_page_neuve ≈ 2 µs (write 4K), update_node_first_prop_off ≈ 1 µs,
> WAL PropSetInline ≈ 1 µs, mécanique commune (intern+effacer+read+lock)
> ≈ 3,5 µs. Écartés après mesure : gc_interval 100→20 (gain sur le chemin
> manager pur ×1,55 mais DANS LE BRUIT sur le GQL réel — reverté) ;
> pré-touch des pages après grow (élimine les copies lentes mais ZÉRO gain
> sur le temps de prop — le coût n'est pas les page faults — reverté).
> Le coût prop restant est structurel (double journalisation WAL
> PropSetInline→DashMap + PropsZone→pages + CRC + locks) — chantier
> substrat profond, post-1.0, hors scale linéaire.

> ✅ **ROUND 5 — LE SCALE GQL B1 EST AU PLAFOND MATÉRIEL (2026-09-06)** :
> mesures propres (benches longs, warmup inclus, 2 runs) :
>
> | Chemin | mono | @8 | Scale | vs plafond |
> |---|---|---|---|---|
> | SELECT pur (machinerie lecture) | 325k/s | 1,53 M/s | ×4,7 | 100 % (plafond) |
> | substrate nu create+prop (8 stores) | 113k/s | 706k/s | **×6,1** | dépasse le SELECT |
> | GQL CREATE 1 prop (B1, 8 graphes) | 60k/s | 260k/s | **×4,0-4,5** | 85-96 % du plafond |
> | GQL CREATE 1 prop en UNE tx/thread | — | 350k/s | — | gap ×1,9 vs crud |
>
> → Le scale GQL B1 (×4,0-4,5) est à **85-96 % du plafond matériel** du
> SELECT pur (×4,7). Le substrate nu dépasse même le SELECT (×6,1) : le
> travail sans machinerie parallélise mieux. Le gap résiduel GQL-vs-substrate
> (×1,9 en une_tx — donc SANS begin/commit par op) = la MACHINERIE par
> statement (parse + plan physique + executor, recréés à chaque
> `execute`), PAS le manager (une_tx ≈ auto-commit, prouvé round 2) ni le
> record_write (0,14 µs tx longue). Le refactor v3 (manager par graphe) ne
> récupérerait que ~10 % — le gap est dans le pipeline GQL par statement,
> un chantier du moteur de requêtes, pas du multi-writer.
>
> ✅ **ROUND 5 — GAIN APPLIQUÉ `f183550d`** : fast-path cache optimisé
> AVANT le parse (+5,5 % mono : 15,17 → 14,33 µs/op CREATE répété ;
> +5 % @8 : 254-266k → 271k/s). Le parse n'est plus refait quand le plan
> optimisé est en cache. Restant : plan physique + executor par statement.

> ✅ **ROUNDS 6-7 — LEVIER 2 + SCÉNARIO B1 RÉEL VALIDÉ (2026-09-06)** :
> - **THP props v2** (`a60b45d7`) : exclusion de MADV_HUGEPAGE sur les
>   zones props (écrites par petits appends, lues par clé) → prop page
>   neuve **8,09 → 7,1 µs (−12 %)**. Suite (b) documentée du pilotage
>   `OBRAIN_TMN_HUGEPAGE`.
> - **GQL B1 après tous les gains** : mono **59-63k → 73k/s** (+20 %),
>   @8 **254-266k → 300-327k/s**. Scale ×4,1-4,5 = **95 % du plafond**
>   matériel (mono 73k × 4,7 = 343k attendu, mesuré 326k).
> - **Scénario B1 réel mesuré** : bulk import 400k nœuds + 1,6M arêtes
>   (0,30 s) EN PARALLÈLE de CREATE GQL runtime sur un autre graphe — le
>   runtime garde **89 % de son débit isolé** (44,6k vs 50k/s isolé ; les
>   ~11 % = partage CPU avec l'import, pas de cross-blocking de stores).
>
> **Conclusion multi-writer** : le partitionnement B1 (N stores = N WAL =
> N write syscalls parallèles) scale au plafond matériel de la machine
> (×4,7-6,1 selon le chemin). Le goal « chaque thread garde sa perf mono »
> est satisfait dans la limite physique : mono 73k × ~4,7 = 343k attendu,
> mesuré 300-327k @8 (87-95 %). Le fix scope B1 (faux conflits 75 % → 0 %,
> round 1) est le prérequis qui a rendu ce scale possible. Levier 2 (coût
> prop) : −12 % livré (THP), le reste est structurel (double journalisation
> WAL), post-1.0.

Porte B1 : import code bulk + écriture runtime simultanés sur deux stores du même
process → aucun cross-blocking, lectures de chaque store non régressées ; crud
disjoint @8 ≥ ×4 le mono (662k/s mesuré), GQL disjoint @8 ≥ ×2,5 le mono (à
atteindre après allègement du manager partagé).

> 📌 **APRÈS GOAL — DÉCOMPOSITION FINALE DE LA MACHINERIE GQL (2026-09-06)** :
> CREATE 1 prop, mono, après tous les gains (cache-parse + THP) :
> auto-commit **13,13 µs/op** = substrate (create+prop) 8,44 + begin+commit
> par op **1,51 µs** + machinerie résiduelle ~3,2 µs (parse sauté + plan
> physique ~0,02 + executor ~0,12 + locks session, dispersés). Verdict :
> aucun poste dominant attaquable SÛREMENT — le begin+commit exigerait le
> refactor manager-par-graphe (échecs mesurés rounds 2-4), le reste est
> dispersé. Le substrate domine désormais le chemin. Levier 2 profond
> restant = double journalisation WAL des props (PropSetInline→DashMap +
> PropsZone→pages) : chantier de SÛRETÉ CRASH (le WAL inline couvre la
> fenêtre entre flushs) → RFC de sûreté dédiée, pas un patch.
>
> 📌 **APRÈS GOAL (suite) — pistes restantes écartées après mesure** :
> (1) cache de plans : `get_optimized` clone le LogicalPlan à chaque hit
> (lock Mutex + clone) — ne scale qu'à ×3 @8 sur SELECT répété, mais le
> workload réel (CREATE à valeurs variables `id:{N}`) est cache-MISS
> systématique (clé = texte complet) → ni sharding ni Arc n'aideraient le
> chemin réel ; le cache-parse (round 5) a déjà capturé le cas texte-constant.
> (2) fsync EveryCommit : non déclenché par CREATE (13 µs/op prouve l'absence
> de fsync par statement — le flag FLAG_COMMIT n'est pas porté par les
> records de mutation). (3) requêtes paramétrées (`$p` +
> execute_with_params) : permettraient au cache de fonctionner sur le
> workload à valeurs variables, mais = changement d'API utilisateur, pas un
> gain automatique. → Aucun levier sûr restant sans RFC de sûreté ou
> changement d'API ; le chemin est dominé par le substrate.

### 3.3 Phase B2/B3 — Vrai multi-writer mono-store (post-1.0, risqué)

- **B2 — WAL multi-têtes** : segments WAL par writer + fusion ordonnée par LSN
  (lsn global = source de vérité de causalité, wal-spec §2), un compactor applique
  aux zones. Le plus séduisant sur le papier, le plus dangereux pour la sûreté mmap.
- **B3 — Writer central (log broker)** : les writers envoient leurs transactions à
  un writer unique qui les sérialise en group commit ; cache local pour
  read-your-writes. Plus simple, garde le modèle « un mutateur de zones », mais le
  broker reste un point de passage (groupé, il encaisse beaucoup).

Recommandation : B3 avant B2 si le besoin mono-store réel apparaît ; B2 seulement
avec un filet complet (oracles, crash tests, bench daté) et un design de fusion
validé sur synthétique avant tout déploiement.

## 4. Invariants à geler par des tests (le filet du chantier)

Chaque invariant ci-dessous doit être couvert par un test AVANT de commencer les
phases A/B — c'est l'objet du présent chantier « tests » :

| # | Invariant sensible | Où | État actuel des tests |
|---|---|---|---|
| I1 | Verrou exclusif refusé si une part partagée vit dans le process | `file.rs` registre | ✅ unit (`verrou_partage_ro_coexiste_et_bloque_rw`, `un_detenteur_exterieur_est_refuse…`) |
| I2 | Deux ouvertures RO du même store partagent la part | `file.rs` | ✅ unit (`deux_ouvertures_du_meme_repertoire…`) |
| I3 | `open_shared_write` + `open_read_only` coexistent et le RO voit les commits | engine + substrate | ❌ **aucun test** |
| I4 | Un 2ᵉ writer exclusif est refusé pendant qu'un writer shared-write vit | engine | ⚠️ partiel (agent_memory_migration `test_multi_process_file_lock_rejected`) — à étendre shared-write |
| I5 | Append WAL : LSN monotone sous threads concurrents, zéro perte | `wal_io.rs` | ✅ 2 tests multithread (`multi_writer_wal.rs`) — **réécrits 2026-10-06** : l'invariant gelé est désormais « ordre physique == ordre LSN », plus « contigu 1..=N » (voir addendum) |
| I6 | Rejeu à l'ouverture : un RO ouvert après N commits voit tout (idempotence, zéro perte) | substrate replay | ✅ partiel (`crash_safety`, `wal_checkpoint`) — à couvrir via engine `open_shared_write`→RO |
| I7 | Write-write conflict : 1ʳᵉ writer gagne, le 2ᵉ est rejouable/aborté | `manager.rs` | ✅ unit + engine (`concurrent_sessions`) — à re-couvrir sur **store substrate persistant** |
| I8 | `TransactionWriteTracker` route node/edge writes → conflit remonté | `write_tracker.rs` | ❌ **0 test** |
| I9 | ParallelExecutor : ordre de commit déterministe, ré-exécution des conflits | `parallel.rs` | ✅ unit (10) — ❌ jamais câblé sur un vrai store |
| I10 | Lecture RO pendant écriture active : pas de blocage, résultats cohérents | substrate | ✅ engine (`test_concurrent_vector_search_during_writes`, concurrent_sessions) — à étendre substrate persistant |

### 4.1 Découverte du filet (2026-09-06, test I5)

> ⚠️ **PÉRIMÉ DEPUIS LE 2026-10-06 (axe APPEND).** Le LSN n'est plus réservé par
> `fetch_add` : il est **dérivé de la position physique** (`lsn = ancre +
> position`, ancre durable dans le side-car `<wal>.lsn`). L'ordre physique **EST**
> l'ordre LSN par construction en mono-écrivain, et la contiguïté `1..=N` a
> disparu avec le compteur par processus. Voir l'**Addendum** en fin de document,
> qui remplace les trois paragraphes ci-dessous pour tout ce qui touche au LSN.

Le test multithread `multi_writer_wal::append_multithread_lsn_contigus_zero_perte` a
révélé un fait de conception que le code ne documentait pas : **sous appends
concurrents, l'ordre PHYSIQUE des records dans le WAL ≠ ordre des LSN**. Cause :
`WalWriter::append` réserve le LSN par `fetch_add` atomique (ordre de réservation)
mais l'écriture est sérialisée par le `Mutex<WriterInner>` (ordre d'acquisition du
mutex) — deux ordres qui peuvent diverger sous contention.

Conséquence pour le multi-writer :
- L'invariant GARANTI aujourd'hui est ensembliste : l'ensemble des LSN émis est
  exactement `1..=N` (aucun trou, aucun doublon), et la relecture retrouve tous les
  records (zéro perte). C'est ce que le filet vérifie.
- L'ordre physique ≠ ordre LSN n'est PAS un bug en mono-writer (un seul writer =
  réservation et écriture séquentielles cohérentes). C'est un point de conception
  à trancher pour la Phase A (group commit : réserver PUIS écrire sous le même
  verrou, ou trier par LSN au merge) et la Phase B2 (WAL multi-têtes : la fusion
  par LSN doit trier, pas supposer l'ordre d'arrivée physique).
- Le replay/`iter_from` s'arrête au premier record corrompu (crash boundary) :
  sous writers concurrents, un lecteur peut voir une queue partielle — jamais un
  record corrompu au milieu (vérifié par le 2ᵉ test, `append_multithread_contre_reader_concurrent`).

## 5. DoD (8 cases) — volet « tests + RFC » (le présent chantier)

1. RFC `docs/rfc/substrate/multi-writer.md` posée (ce document), additif (aucun
   format/module existant modifié). ✅
2. Tests I3/I4/I6 : fichier d'intégration engine `multi_writer_invariants.rs`
   (open_shared_write → RO voit les commits après réouverture ; RO pendant writer
   actif ; 2ᵉ writer refusé ; rejeu idempotent à l'ouverture). ✅ 4 tests
3. Tests I5 : append WAL multithread (LSN ensemble 1..=N sans trou, zéro perte,
   lecteur pendant écriture sans corruption) — `multi_writer_wal.rs` (substrate). ✅ 2 tests
4. Tests I7/I8 : write-write conflict + retry sur **store substrate persistant**
   (`conflit_write_write_sur_store_substrate`) ; `TransactionWriteTracker` →
   `WriteConflict` remonté en `OperatorError` (3 tests unitaires). ✅
5. Tests I9 : ParallelExecutor exécuté contre un vrai store (12 CREATE réels via
   l'API crud + read/write sets enregistrés), zéro perte après réouverture. ✅ 1 test
6. Suites : engine lib **683/683** (680 + 3 write_tracker) · substrate lib **807/807** ·
   tests d'intégration nouveaux **7/7 engine + 2/2 substrate** — aucune suite
   existante cassée (inventaire 0 rouge). ✅
7. Bench daté : AUCUN chemin chaud touché (tests + RFC additifs stricts) — bench
   non requis pour ce volet ; à faire pour les phases A/B (avant/après).
8. Ledger + commit unique réversible + cette RFC. ✅ (voir ledger 2026-09-06)

## 6. Limites / ouvertures (honnêtes)

- Phase B (multi-process mono-store) NON engagée ici : le présent chantier pose la
  RFC + le filet de tests. Les phases A/B1/B2/B3 sont des chantiers distincts avec
  leur propre DoD et bench.
- Le verrou `LOCK_SH` de `open_shared_write` ne garantit PAS l'exclusion entre deux
  writers (convention appelant) : documenté I4, à résoudre en phase B — ne pas
  l'oublier dans le design B2/B3 (les writers doivent se coordonner sur l'append,
  pas sur le verrou de répertoire).
- Rattrapage de queue pour les lecteurs live (au-delà du rejeu à l'ouverture) :
  mécanique `cdc_tail` à généraliser — chantier séparé, hors périmètre tests.
- Le test multi-process réel (2 processus partageant un store) est hors portée des
  tests unitaires classiques — couvert par I4 en simulacre (2ᵉ ouverture refusée
  dans le process) + porte manuelle `verifier-deploiement` sur stores réels.

## 7. Ordre d'exécution

1. Tests d'invariants (ce chantier) : I3/I4/I6 engine → I5 substrate → I7/I8/I9
   engine → suites complètes → ledger + commit.
2. Phase A (chantier séparé) : brancher ParallelExecutor + group commit → bench.
3. B1 puis B3/B2 (chantiers séparés, décision utilisateur).


---

## 8. ADDENDUM — AXE APPEND : LA FIFO MULTI-ÉCRIVAINS (2026-10-06)

**Statut** : implémenté, testé, mesuré. Ce chapitre REMPLACE, pour tout ce qui
touche au LSN et à l'atomicité de l'append, les affirmations des §3.1ter, §4 et
§4.1 de ce document.

### 8.1 Ce qui était faux

| Affirmation d'origine | Réalité mesurée |
|---|---|
| « `WalWriter::open` écrit en ajout » (`wal-spec.md` §31) | **Faux** : le fichier était ouvert `.write(true)` SANS `O_APPEND`. Deux processus écrivaient donc à LEUR offset de descripteur, et le second écrasait le premier frame par frame, avec des CRC tous valides. |
| « l'ordre physique du WAL n'est PAS l'ordre LSN » | **Vrai avant**, faux après : `lsn = ancre + position` fait de l'ordre physique l'ordre LSN. |
| « l'ensemble des LSN émis est exactement `1..=N` » | **Faux après** : le LSN n'est plus un compteur. En mono-écrivain il vaut `ancre + position` ; en multi-écrivain c'est une **borne inférieure** de la position (régime `OAppend`). |
| « chaque append fait un `write_all` » | **Vrai avant**, faux après : UN SEUL `write`, et une écriture courte est ANNULÉE par troncature (une frame hybride est indécodable). |

### 8.2 Les six pièces

1. **Ancre de LSN durable** — side-car `<wal>.lsn` (32 octets : magie `OBLA`,
   version, `ancre: u64`, `plancher: u64`, CRC), écrit par tmp+rename.
   `WalWriter::new` ne sème plus `next_lsn` par processus (`writer.rs`).
2. **LSN = ancre + position** — la position est l'offset octet de la frame.
3. **Réconciliation d'ouverture** — trois cas NOMMÉS : `AncreAdoptee`,
   `ReancrageEnAvant` (side-car absent sur un WAL non vide : l'ancre est
   ré-ancrée au-delà du plus grand LSN mesuré), `TroncatureEtrangere` (fichier
   plus court que le plancher du side-car).
4. **Checkpoint ordonné** — `WalWriter::truncate()` publie l'ancre AVANT de
   tronquer (l'ordre est dans la fonction, pas chez l'appelant) puis
   `sceller_segment()` relève le plancher.
5. **Garde d'append noyau** — la taille RÉELLE est relue sous le mutex ;
   plus grande → recalage, plus petite → troncature étrangère + relecture de
   l'ancre.
6. **Lecteur qui refuse les trous** — une plage de zéros SUIVIE DE DONNÉES rend
   `SubstrateError::WalTrou { offset, longueur }` (nommée), au lieu de passer
   pour une fin de fichier ou une frontière de crash.

### 8.3 Coordination de l'append : le compromis mesuré

`CoordinationAppend` (voir `wal_io.rs`) a trois régimes. `Writer::new` choisit
`Aucune` quand le verrou de RÉPERTOIRE est exclusif (le cas normal : personne
d'autre ne peut écrire), `OAppend` quand il est partagé.

| régime | non-perte | LSN | débit cumulé à 8 processus |
|---|---|---|---|
| `Aucune` (verrou exclusif) | `O_APPEND` | **exact** | chemin chaud inchangé |
| `OAppend` (défaut partagé) | `O_APPEND` | **borne inférieure** | **×4,7 à ×5,1** |
| `VerrouExact` (`flock`) | `O_APPEND` + `flock` | **exact** | **×0,64** |

**Décision** : `OAppend` par défaut, parce que la FIFO doit SCALER et que la
non-perte ne dépend pas du verrou. Le prix est nommé : en multi-écrivain, le LSN
estampillé peut désigner une position antérieure à la sienne (mesuré : ~96 % des
frames dans T2, à 6 processus). Ce n'est PAS une perte — l'ancre du checkpoint
avance à `ancre + taille`, donc strictement au-delà de toute borne inférieure, et
aucun LSN n'est jamais réémis. `VerrouExact` rend le LSN exact pour qui en a
besoin.

### 8.4 Filet (tous verts)

`tests/mw_interprocess_preuve.rs` (T2 forme dure — **rouge avant : 83,3 % de
perte**), `tests/mw_interprocess_append.rs` (T2, T2b, T3, T4, T7, débit),
`tests/wal_append_unite.rs` (T6, T7, borne de taille, ENOSPC, réconciliation,
trou), `tests/wal_ecriture_courte.rs` (quota disque réel),
`tests/wal_latence_append.rs` (non-régression du chemin chaud).
