# RFC — Double journalisation WAL des propriétés : éliminer le doublon (chantier de sûreté)

> **AMENDEMENT DE VALIDATION — 2026-09-06** (revue code + graphe, option C retenue).
> La version initiale (945feb65) est amendée par la revue de validation croisée
> code (`store.rs`, `writer.rs`, `replay.rs`, `wal.rs`, engine, migrate) et
> graphe (nœud `plan:double-journalisation-wal-props`). Modifications :
> (1) §2 reformulé — le modèle de sûreté invoqué (le replay de `PropSetInline`
> restaurerait les valeurs de la fenêtre entre flushs) **n'est vrai que pour le
> régime hérité** : sous v2, `merge_wal_inline_props` saute déjà le replay des
> valeurs (`store.rs:3650-3653`), la fenêtre est couverte par la page cache
> mmap (périmètre documenté « kill de processus », `store.rs:20512-20516`) ;
> (2) l'ordre « WAL→mmap structurel » ne s'applique pas au record de VALEUR
> (pas une primitive de réparation de slot) — seul `NodePropHeadUpdate` l'est ;
> (3) §3 répondu (cartographie faite) ; (4) option **C (repli conditionnel)**
> retenue, périmètre nœuds **+ arêtes**, contrat de durabilité tranché ;
> (5) DoD reformulé. Preuves : 3 inventaires de lecture (migration/régimes,
> moteur+invariants, spec/format/perf) — aucun fichier de code modifié par la
> revue.

> **LIVRÉ — 2026-09-06, option C implémentée et vérifiée.** Commits :
> RFC amendée `ed8e5bd5` · code + preuves + sonde `8d710b99` · ledger
> `2cd35082`/`15fb56a0`. DoD vert : substrate lib **810/810** (dont 2 preuves
> de non-perte `*_regime_v2` : WAL à ZÉRO record de la famille + état identique
> au reopen), engine lib **683/683**, core **870/870**, invariants **9/9**,
> intégration (multi_writer_wal/crash_safety/wal_checkpoint) et t17k verts.
> A/B mono (sonde `perf_prop_wal`, 2 runs/état, avant = store.rs pré-option C
> `ed8e5bd5`) : prop page neuve **6,88 → 5,70 µs (−17 %)**, prop page chaude
> **2,03 → 0,80 µs (−61 %)**, WAL allégé (base perf_churn 24,2 → 23,6 Mo).
> `SUBSTRATE_FORMAT_VERSION` inchangé. Suite du chantier ouvert : RFC « pages
> PropsZone partagées » (densité 4K/chaînage), décidée avec l'utilisateur.

**Statut** : **LIVRÉE** (option C en place) — chantier de sûreté crash,
seul levier structurel restant sur le chemin d'écriture des propriétés après
le goal multi-writer (complete, 8 rounds). Règle stricte respectée :
preuve de non-perte AVANT le code — le code du DoD (option C) suit cet
amendement, jamais l'inverse.

**Date** : 2026-09-06 — branche `feat/zorder` — HEAD de référence `945feb65`
(RFC initiale).

---

## 1. Le problème mesuré

Chaque `set_node_property` scalaire écrit la valeur **DEUX fois** sur deux
chemins de durabilité différents (vérifié à HEAD, 4 sites producteurs
seulement) :

1. **append chaîne PropsZone v2** (pages mmap + CRC32) — `append_scalar_to_props_zone_v2`
   (`store.rs:7108`), appelé par `set_node_property` (`store.rs:15409-15410`).
2. **append `WalPayload::PropSetInline`** (WAL) — `store.rs:15445`, émis
   **inconditionnellement aujourd'hui**, y compris quand la zone v2 a pris
   l'écriture.

Plus, ponctuellement : append `KeyIntern` (1ʳᵉ occurrence de la clé,
`store.rs:15363`/`7068`) et append `NodePropHeadUpdate` **seulement quand la
tête de chaîne change** (1ʳᵉ prop d'un nœud ou nouvelle page) — `store.rs:7151-7152`
→ `writer.rs:341-352`. En régime établi (prop sur nœud chaud), le doublon =
**1 append** (`PropSetInline`), pas 2. Le miroir DashMap est déjà conditionnel
depuis le 19/08 (skip quand `v2_a_pris && !hydratee && pas d'index`,
`store.rs:15567-15571`) — **précédent exact du gating proposé ici**.

**Suppressions — jumeaux** : `remove_node_property` → tombstone chaîne v2
(`store.rs:15758-15767`) + `PropRemoveInline` WAL (`store.rs:15771`) ;
`remove_edge_property` → `EdgePropRemoveInline` WAL (`store.rs:15989`) +
tombstone chaîne arêtes. **Arêtes — jumeaux des nœuds** :
`set_edge_property` (`store.rs:15592`, `EdgePropSetInline` `15661`),
`append_scalar_to_props_zone_v2_edge` (`7433`), `EdgePropHeadUpdate`
(`writer.rs:945`), `remove_edge_property` (`15953`). **La présente RFC couvre
les nœuds ET les arêtes.**

**Hors périmètre** : `Value::Vector` et chaînes/octets longs routés colonnes
(partent AVANT le chemin v2, `store.rs:15368-15381`) — aucun record `*Inline`
pour eux ; les records legacy `PropSet 0x20`/`PropDelete 0x21` ne sont plus
écrits (tests seulement, `wal.rs:890/985`).

**Coût mesuré** (rounds 3-7, sondes datées — l'instrumentation par étape ne vit
dans AUCUN bench rejouable, cf. §5) : prop page neuve = **7,1 µs** après le
gain THP (8,09 µs avant), décomposition (multi-writer.md:563-568) :
allouer_page_neuve ≈ 2 µs, update_node_first_prop_off ≈ 1 µs,
**WAL PropSetInline ≈ 1 µs**, mécanique commune ≈ 3,5 µs. CREATE 1 prop mono =
**13,13 µs** = substrate 8,44 + begin/commit 1,51 + machinerie 3,2.
L'append WAL = 1 encode (~0,005 µs depuis `encode_into`, 254f57fd) + 1 write
syscall (~0,94 µs, structurel, wal_io.rs:125-144). Le doublon est donc payé à
chaque prop : ~1 µs de syscall + encodage + volume WAL (fenêtre rejouée à
l'ouverture, ~3 ms/Mo mesuré).

## 2. Pourquoi c'est un chantier de SÛRETÉ — le modèle EXACT (amendé)

**Le risque central reformulé.** Le record `PropSetInline` ne sert, au rejeu,
**que** les bases où la DashMap fait foi : régime hérité (`OBRAIN_PROPS_V2=0`,
~1 528 bases non migrées), bases v2 non finalisées (marqueur absent) et repli
anti-refus. Sur une base v2 finalisée (`v2_has_pages` à l'ouverture),
`merge_wal_inline_props` **saute le replay des valeurs** (`store.rs:3650-3653`,
3671-3673, 3685-3687, 3698-3700) : il ne relève que filigranes (`3647-3649`)
et registres `*Intern` (`3723-3731`) — indispensables pour traduire
`prop_key_id` → nom, mais indépendants des valeurs `PropSetInline`. Le record
de valeur est donc **redondant sous v2** ; le retirer n'y change aucune fenêtre
de crash.

**Fenêtres et modèle documenté.** Périmètre officiel = **kill de processus** :
les pages mmap (zone v2, têtes, slots) survivent via la page cache OS ; le
replay à l'ouverture (`replay_from` depuis `wal_offset_at_flush`,
`store.rs:4344-4352`) ré-applique les records zone (NodeInsert, têtes…) de
façon idempotente, et `merge_wal_inline_props` restaure registres et
filigranes. « Une coupure de courant, elle, dépend du writeback OS pour les
octets de zone — périmètre documenté » (`store.rs:20512-20516`). Sous v2, la
fenêtre entre flushs est couverte par la **page cache mmap**, pas par les
valeurs `PropSetInline` (dont le replay est sauté) — et c'est le
**contrat de durabilité que cet amendement tranche** :
le périmètre documenté (process-crash garanti ; coupure de courant = writeback
OS best-effort) est accepté pour ce chantier. Aucune option ci-dessous ne
change la garantie coupure de courant (déjà non fournie par `PropSetInline`
sous v2, rejeu sauté).

**L'ordre WAL→mmap — où il est structurel, et où il ne l'est pas.** La
contrainte qui a invalidé le group-commit d'appends (multi-writer.md §3.1ter,
lignes 210-243) vaut pour les primitives dont le rejeu **répare des slots
mmap** : `write_node`/`write_edge`/`update_*`/`update_*_first_prop_off` —
append WAL d'abord, mutation mmap ensuite, rejeu idempotent. C'est le cas de
`NodePropHeadUpdate` (`writer.rs:342-352`, rejoué `replay.rs:1229-1248`, prouvé
par `update_node_first_prop_off_replays_after_mmap_wipe`, `writer.rs:3791`).
Ce n'est **pas** le cas du record de VALEUR `PropSetInline` : son rejeu vers la
zone n'existe par aucun chemin (skip sous v2 ; `Skipped` par contrat dans
`replay.rs:1302-1305`), et dans le code son append suit d'ailleurs l'écriture
zone (`store.rs:15409-15410` puis `15445`). Les records à préserver sous v2
sont donc : `NodePropHeadUpdate`/`EdgePropHeadUpdate` (têtes, WAL-first),
`*Intern` (registres), et les filigranes — **pas les valeurs `PropSetInline`**.

**Aucun autre journal durable de la valeur n'existe** (inventaire moteur) :
le WAL moteur legacy (`LpgWal`, `WalRecord::SetNodeProperty`) est
structurellement `None` sur tous les chemins substrate réels
(`open_substrate`/`open_shared_write` → `with_store`, `mod.rs:1063-1064` ;
bloc `LpgWal::with_config` `mod.rs:858-908` = code mort court-circuité) ;
reste le CDC (`substrate.cdc_journal`, zone dédiée — valeur avant+après, chemin
crud uniquement, fail-soft) et le journal d'annulation (`avant`, RAM,
`store.rs:1448-1458`) — **aucun n'est un redo de l'état des zones**, aucun
n'est affecté par le retrait conditionnel.

## 3. Cartographie (faite — réponses aux quatre questions)

1. **Qui lit `PropSetInline` au replay ?** Un seul rejoueur :
   `merge_wal_inline_props` (`store.rs:3527`, appelé `4714`). Sous
   `v2_has_pages` (`3579-3583` = pages de zone allouées) : les valeurs sont
   **sautées** (`3650-3653`) ; seuls filigranes + `*Intern` sont relevés.
   `replay.rs` déclare les quatre `*Inline` `Skipped` par contrat de
   délégation (`1302-1305`, liste exhaustive). → **Les valeurs ne servent que
   les bases héritées / non finalisées / refus-v2** (sans pages v2 au reopen).
2. **Qui lit la PropsZone v2 ?** Ordre LWW : vec → blob → chaîne v2
   (`lookup_node_property_v2`, `store.rs:7366+`) → DashMap **en filet
   seulement si la chaîne n'a pas d'entrée** (`lire_propriete_noeud_vivant`,
   `11658-11667` ; jumeau arête `11704-11708`). La chaîne est l'autorité de
   lecture sous v2.
3. **Fenêtres de crash** : flush → `Writer::commit()` + `msync_zones`
   (`store.rs:5625-5626`) ; checkpoint auto (seuil `OBRAIN_WAL_CHECKPOINT_BYTES`
   8 Mio, `store.rs:4279-4292`, troncature checkpoint.rs:53-132) ; rejeu
   d'ouverture borné à la fenêtre depuis le dernier flush (`wal_offset_at_flush`).
   Pas de fsync par statement : records de mutation `flags:0`, fsync seulement
   sur le marqueur de commit posé par `Writer::commit()` (writer.rs:2838-2847 ;
   wal_io.rs:146-153) — preuve : 13,13 µs/op (multi-writer.md:646-648).
   Reconstruction kill-9 sans flush sous v2 = **page cache mmap** (éprouvée par
   les 6 scénarios `*_regime_v2`, `store.rs:20451-20772`) ; la reconstruction
   des VALEURS depuis le WAL après wipe de zone n'existe **nulle part**
   (crash_safety.rs ne reconstruit que des `NodeInsert` ; les rebuilds
   post-wipe = têtes writer.rs:3791, engram, simplex).
4. **Rôle de `NodePropHeadUpdate`** : primitive **WAL-first** du head pointer
   v2 (6 o, `writer.rs:342-352`), rejouée dans le slot nœud à l'ouverture
   (`replay.rs:1229-1248`), no-op toléré si le slot n'est pas matérialisé.
   **Elle reste obligatoire** (avec `*Intern` et filigranes) si la PropsZone
   devient/se maintient seule source — contrairement aux valeurs `PropSetInline`.

## 4. Options — départage et décision (amendé)

- **C. Repli conditionnel — RETENUE.** N'appender `PropSetInline`/
  `PropRemoveInline`/`EdgePropSetInline`/`EdgePropRemoveInline` que quand la
  zone v2 n'a **pas** pris l'écriture (`!v2_a_pris` pour les sets ; succès du
  tombstone de chaîne pour les removes). Miroir exact du gating DashMap du
  19/08 (`15567-15571`) et du filet anti-refus (`v2_appends_refuses`,
  `15416`/`15646` ; repli DashMap conservé sur refus). Régime hérité
  (`props_zone == None` → `v2_a_pris` faux) **inchangé bit pour bit**.
  Zéro changement de format, réversible, ~5 lignes par site (4 sites). Gain :
  −1 encode −1 write syscall (~1 µs) par prop scalaire sous v2 + WAL allégé
  (volume rejoué à l'ouverture). Risque nul dans le périmètre documenté
  (prouvé par la preuve §6.2 : le replay des valeurs est déjà sauté sous v2).
- **A. Suppression pure sous v2** : dominée par C — elle perdrait le filet sur
  un refus v2 ponctuel (valeur exotique non routée colonne).
- **B. Record fusionné (prop + tête)** : nécessiterait un nouveau kind en queue
  d'enum (serde par indice, wal.rs:438-443 ; kinds fixes wal.rs:66-76) — les
  deux records ont des rôles et des moments différents (valeur à chaque prop ;
  tête seulement au changement de tête) : pas de bénéfice net. Écartée.
- **D. Ne rien faire** : filet si la preuve de non-perte échoue.

## 5. Garde-fous (actualisés à HEAD)

- Sites de code : `store.rs` `set_node_property` (15348), `set_edge_property`
  (15592), `remove_node_property` (15695), `remove_edge_property` (15953) ;
  appends aux lignes **15445 / 15661 / 15771 / 15989** ; zone v2
  `append_scalar_to_props_zone_v2` (7108) / `_edge` (7433) / tombstones
  (7311/7487) ; rejeu `merge_wal_inline_props` (3527) ; `replay.rs` `Skipped`
  (1302-1305) ; `Writer::append_payload` (writer.rs:376).
- Autorité réelle de régime = **marqueur** `migration_v2_achevee` (5889),
  posé uniquement à `delete_legacy_props_sidecar` (6172) ; décision v2 à
  l'ouverture (4087-4163, base neuve testée par la TAILLE du sidecar — leçon
  `a4c1edaf`) ; hydratation des DashMaps `load_properties` (5000, flags
  `4809-4814`, lus `15568`/`15684`) ; drains `finalize_props_v2` (6624, filet
  `v2_appends_refuses` 6765-6803) / `finalize_edge_props_v2` (6909 — **sans**
  ce filet : asymétrie constatée, doc interne ambiguë 6966+, à corriger dans un
  commit séparé).
- Filet de tests anti-régression du rejeu : 6 paires `*_regime_herite` /
  `*_regime_v2` (`store.rs:20494-20772` : kill sans flush — prop, nœud
  zero-flush, last-write-wins, non-résurrection delete nœud/arête, prop
  d'arête) ; tests marqueur/env/hydratation/finalize/reprise (~40,
  inventoriés en revue) ; `crash_safety.rs` (torn WAL + wipe zones) ;
  `multi_writer_wal.rs`, `multi_writer_invariants.rs` (I12 = import bulk
  400k/1,6M, repère 0,22 s).
- Moteur : pas de 2ᵉ WAL (cf. §2) ; CDC et journal d'annulation non affectés.
- Incohérences doc relevées (corrections hors périmètre, à faire) :
  commentaire fsync `store.rs:3517-3519` (contredit la cadence réelle) ;
  file.rs:584 (6,89 µs/−15 %) vs docs (7,1 µs/−12 %) ; wal-spec.md §3 périmé
  (kinds 0x22-0x27 absents).

## 6. DoD (définition de done — reformulée)

1. RFC amendée validée (présente), cartographie §3 répondue, option C retenue,
   contrat de durabilité tranché (§2).
2. **Preuve de non-perte** : tests de crash (kill simulé sans flush, comme les
   scénarios existants) avec props écrites entre flushs **sans record
   `*Inline` dans le WAL** → réouverture → état identique (count + valeurs) —
   sous option C, pour nœuds et arêtes ; les 12 tests `*_regime_*` existants
   restent verts (ils ne dépendent pas des valeurs sous v2).
3. Toutes suites vertes : substrate lib (808 + nouveaux), engine lib 683,
   core 870, invariants (**9 tests** dans `multi_writer_invariants.rs` — noter
   que I1/I2/I8 n'ont pas de test nommé : la mention « 9/9 (I1-I12) » est
   trompeuse et abandonnée), `crash_safety`, `wal_checkpoint`, `multi_writer_wal`,
   t17k v2.
4. Bench daté avant/après : harness `phase_a_write_goulot` (mono, CREATE 1 prop
   sur store substrate réel — le seul bench rejouable couvrant le chemin) +
   repère I12 (import bulk) ; le micro-µs par étape reste hors bench (sondes
   datées — documenté, voir revue).
5. `SUBSTRATE_FORMAT_VERSION` (= 1, meta.rs:8) inchangé — le gating ne change
   ni kind ni contenu de record.
6. Ledger + commit(s) réversibles, 0 rouge.

## 7. Contexte tissé dans le graphe

Nœuds `Plan` liés aux symboles (CONCERNS) : `set_node_property`,
`set_node_property_versioned`, `append_scalar_to_props_zone_v2`,
`merge_wal_inline_props`, module `store`. Clé : **`double-journalisation-wal-props`**.
La revue de validation (3 inventaires de lecture + vérifications directes) est
documentée dans la session ; aucun artefact graphe nouveau requis pour la
validation.
