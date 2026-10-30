# RFC v2 — Principal natif : l'identité tenant descend dans le substrat

**Statut** : brouillon v2 — conception seulement, aucune ligne de production touchée.
**Date** : 2026-09-12 — branche `docs/principal-natif-v2` (worktree `banc-obrain-rfc2`),
dérivée de HEAD `afbb5cfc`.
**Auteur** : harness-RFC-V2bis (dispositif Ergaster), chantier `PRINCIPAL-NATIF-V2`.
**Remplace** : `docs/rfc/principal-natif.md` (v1, commit `57353aac`, branche
`docs/principal-natif`), après revue croisée adversariale par trois harness
(SCAN-PLAFONNE, AGREGATS-O1, IAM-VERROUS) — synthèse : note PO `3cda8545`.

**Convention de lecture.** Chaque affirmation sur le code existant est marquée
**[VÉRIFIÉ fichier:ligne]** et provient d'un `grep`/`sed` lancé sur `ffs-n3`
**pendant la rédaction de ce v2**, sur l'arbre canonique
`~/projects/banc-obrain` @ `afbb5cfc`. Ce qui n'existe pas encore est marqué
**[PROPOSÉ]**. Une mesure reprise d'un audit antérieur, non reproductible par
grep, est marquée **[MESURÉ]** avec sa source. Sauf mention contraire,
`store.rs`, `record.rs`, `meta.rs`, `file.rs`, `warden.rs` désignent
`crates/obrain-substrate/src/*`.

---

## 0. Pourquoi un v2 — et ce que la re-vérification a établi

Le RFC v1 a été challengé sur pièces par les trois harness qui avaient audité le
substrat de première main. Verdict (note PO `3cda8545`) : **thèse validée, trois
décisions sur cinq cassées**. Ce v2 intègre les cinq arbitrages pris par le
superviseur et re-vérifie chaque fait au code — car deux harness se sont égarés
le même jour (arbre fantôme, mauvaise machine) et l'un a déclaré « confabulés »
des symboles qui existent bel et bien.

Relevé de re-vérification (tout re-grep-é aujourd'hui) :

- `noeud_visible_pour` **existe** [VÉRIFIÉ store.rs:3810], `visibilite_triviale`
  **existe** [VÉRIFIÉ store.rs:3824]. Le chemin rapide est consulté à **trois
  sites** : le filtre par lot [VÉRIFIÉ store.rs:3787], `is_node_visible_at_epoch`
  [VÉRIFIÉ store.rs:13082], `is_node_visible_versioned` [VÉRIFIÉ store.rs:13094].
  (Les numéros 3389/12531/12538 cités par la revue ont dérivé ; les symboles sont
  les mêmes.)
- `access_context: parking_lot::Mutex<Option<Arc<AccessContext>>>` [VÉRIFIÉ
  obrain-engine/src/session/mod.rs:124] et `set_access_context` **public**
  [VÉRIFIÉ session/mod.rs:362-363]. Le champ est **mutable après construction**.
- `AccessContext { reader, decide, audit, current_query }` — **pas de tenant**
  [VÉRIFIÉ obrain-core/src/execution/base_registry.rs:246-252].
- `SUBSTRATE_FORMAT_VERSION: u32 = 1` [VÉRIFIÉ meta.rs:8], validé à **chaque
  ouverture** : `open_inner` → `validate_header` [VÉRIFIÉ file.rs:913] qui rend
  `SubstrateError::UnsupportedVersion(trouvée, attendue)` [VÉRIFIÉ
  file.rs:939-944]. **Un binaire v2 refuse proprement un store v1** : la
  migration est un chemin de code au démarrage, pas une bombe de corruption.
- `NodeRecord` = **32 B pleins, align 8**, assertions de compilation [VÉRIFIÉ
  record.rs:92-93, re-testé record.rs:539/541] ; `NODES_PER_PAGE: u32 = 128`
  [VÉRIFIÉ record.rs:100] avec l'assertion `4096/SIZE == NODES_PER_PAGE`
  [VÉRIFIÉ record.rs:103]. `node_flags` occupe les **bits 0-5**
  (`TOMBSTONED`..`IDENTITY`) — **pas de drapeau ALLOCATED** [VÉRIFIÉ
  record.rs:61-69].
- `EdgeRecord` = 36 B align 4 (bump T17f 32→36) avec `_pad: [u8; 2]` **libre**
  [VÉRIFIÉ record.rs:306] — un tenant d'arête u16 y logerait ; réservé, non
  câblé (arbitrage A1, §2).
- **Trous d'allocation** : deux sites détectent les créneaux jamais écrits par
  l'heuristique « tout à zéro » incluant `flags == 0` [VÉRIFIÉ warden.rs:161-169
  ; store.rs:9082-9094]. Mesure d'origine : 10 nœuds créés, 132 slots lisibles
  [MESURÉ, audit AGREGATS-O1, note PO `3cda8545`]. Un slot troué se lit comme
  `NodeRecord::default()` **non-tombstone** — et `create_node_in_community(&[], 0)`
  est byte-pour-byte identique à un trou (le commentaire du site le dit lui-même
  [VÉRIFIÉ store.rs:9078-9089]).
- **Les primitives de scan ne passent PAS par `noeud_visible_pour`** :
  `node_ids` [VÉRIFIÉ store.rs:13633] et `nodes_by_label_capped` [VÉRIFIÉ
  store.rs:13674] lisent la zone mmap via `for_each_node_record_annulable` et
  filtrent en ligne sur le seul bit `TOMBSTONED`. `node_count` = atomique
  `total_live_nodes` [VÉRIFIÉ store.rs:13723-13725], `node_count_by_label` =
  atomiques `label_live_counts[bit]` [VÉRIFIÉ store.rs:13733-13741]. Compteurs
  **globaux, tous tenants confondus**.
- `edges_from` ne lit que la chaîne d'arêtes, jamais l'extrémité lointaine
  [VÉRIFIÉ store.rs:13481-13521] (la revue citait 12931 — dérive de lignes).
  Les surcharges de visibilité **arête** se réduisent à la vivacité : pas de
  carte `en_attente_aretes` [VÉRIFIÉ store.rs:13074-13078, commentaire
  explicite].
- **Fait nouveau, découvert pendant ce v2** : `replay_from()` n'a **aucun
  appelant en production** — constat documenté deux fois dans le code même
  [VÉRIFIÉ store.rs:841-844 et :877-879]. En revanche des passes de fusion
  **lisent** le WAL à l'ouverture (`merge_wal_inline_props` [VÉRIFIÉ
  store.rs:4158], `merge_wal_external_edges` [VÉRIFIÉ store.rs:4569]). La
  formule « la migration passe le WAL » de l'arbitrage A2 se précise donc :
  **drainer/fusionner le WAL v1 AVANT la réécriture, puis le tronquer** (§3).
- Adressage des slots **linéaire** : `offset = slot * NodeRecord::SIZE`
  [VÉRIFIÉ store.rs:8978-8979, :9023-9024] ; `NODES_PER_PAGE` ne sert qu'aux
  décisions d'allocation/compaction/prefetch [VÉRIFIÉ warden.rs:14, :59 ;
  store.rs:11347-11372]. Conséquence chiffrée du bump 32→40 en §4-J2.

Consommateurs, état des lieux :

- **this-rs** (`~/projects/this-rs`) : les liens applicatifs sont des **nœuds**
  de label `_link:{link_type}`, pas des arêtes [VÉRIFIÉ
  this/src/storage/obrain.rs:77, :554-555, :975]. Ouverture par
  `ObrainDB::open` / `open_read_only` [VÉRIFIÉ obrain.rs:130-131, :151-153].
  L'identité vient d'un JWT : `StsClaims { sub, tenant_id: Uuid, roles, ... }`
  [VÉRIFIÉ this/src/core/auth/sts.rs:16-22]. Dépendances par chemin vers
  `../../banc-obrain/crates/*` [VÉRIFIÉ this/Cargo.toml:98-116]. **Constat
  d'exploitation : `~/projects/this-rs` n'est pas un dépôt git** (vérifié sur
  ffs-n3) — à corriger avant toute migration pilotée par lui.
- **obrain-tmn** (`~/projects/obrain-tmn`) : ouvre ses stores par
  `ObrainDB::open_substrate` [VÉRIFIÉ src/obrain_db_pool.rs:569, :1271] et
  `SubstrateStore::open` [VÉRIFIÉ src/exocortex_bridge.rs:1292]. **Constat
  bloquant : ses dépendances pointent vers
  `../../lab/grafeo-pillars/crates/*`, chemin INEXISTANT sur ffs-n3**
  [VÉRIFIÉ Cargo.toml:13-21 ; `ls` négatif]. Le binaire déployé date d'un
  checkout disparu ; il faut re-pointer avant de parler de v2.
- **viz :8432** : port par défaut 8432 [VÉRIFIÉ
  obrain-cli/src/commands/viz.rs:207], ouverture `open_read_only` par un cache
  de handles `cached_ro` **sans aucune identité** [VÉRIFIÉ
  obrain-cli/src/viz_serve.rs:31-44], bind `127.0.0.1` par défaut mais `--host`
  expose [VÉRIFIÉ viz_serve.rs:10, doc de module].

**Aucun fait du brief ne s'est révélé faux** ; trois se sont précisés (dérive de
numéros de ligne ; « passe WAL » à préciser en drainage ; chemin mort
d'obrain-tmn).

---

## 1. Thèse (inchangée du v1, validée par les trois revues)

obrain possède une identité de lecteur (`AccessContext`) mais elle n'est câblée
que sur une frontière (la résolution inter-bases) et à une granularité
grossière (par slug de store, pas par nœud). Toute lecture locale d'un store
unique — le cas de la viz, le cas dominant — ne la traverse jamais, et
`AuthenticatedSession` (obrain-iam) est un compagnon que le middleware doit
*penser* à consulter : une convention, pas une garantie. **Toute garde posée
au-dessus du substrat fuit par accès direct. L'isolation doit descendre là où
passe chaque lecture de nœud.**

Le v1 plaçait ce « là » dans le prédicat de visibilité (`noeud_visible_pour`).
La revue croisée a montré que c'est le mauvais étage : les scans de masse ne
passent pas par ce prédicat [VÉRIFIÉ store.rs:13633, :13674]. Le v2 corrige :
**le tenant est une colonne du `NodeRecord`, testée dans la même boucle mmap
que le bit `TOMBSTONED`** — l'étage en dessous, celui que tous les chemins
traversent.

---

## 2. Les cinq arbitrages (pris par le superviseur, justifiés au code)

### A1 — Cascade par nœud ; PAS de tenant d'arête maintenant

Les « liens » applicatifs de this-rs sont des **nœuds** `_link:{link_type}`
[VÉRIFIÉ this/src/storage/obrain.rs:77] : ils reçoivent donc un `tenant_id`
comme tout nœud, et le confinement des liens applicatifs tombe **gratuitement**
avec le filtre nœud. Les arêtes substrat restent sans tenant ; la restitution
d'une arête est conditionnée à la visibilité de ses **deux extrémités**
(cascade). Le `_pad: [u8; 2]` d'`EdgeRecord` [VÉRIFIÉ record.rs:306] est
**réservé** pour un futur tenant d'arête u16 — documenté, non câblé.

**Limite documentée (M2+)** : la métadonnée d'une arête entre deux nœuds
visibles (type, poids, propriétés) n'est pas confinable indépendamment des
extrémités. Tant que les deux bouts sont d'un même tenant, la cascade suffit ;
une arête inter-tenant (qui ne devrait pas exister sous isolation stricte)
serait invisible dès qu'une extrémité l'est — fail-closed.

Coût de la cascade : `edges_from` ne lit aujourd'hui que l'`EdgeRecord`
[VÉRIFIÉ store.rs:13481-13521] ; vérifier l'extrémité lointaine ajoute une
lecture de `NodeRecord` par arête. Avec le tenant en colonne (lecture ~ns,
comme `label_bitset`), ce coût est du même ordre que le test tombstone déjà
payé — c'est précisément pourquoi M0-propriété est sauté (A2).

### A2 — UNE seule migration de format : 32→40 B, `tenant_id` + `ALLOCATED` ensemble

Le M0 du v1 (tenant en propriété, filtrage au niveau moteur) est **sauté** :
il coûtait des µs/arête en traversée et n'aurait rien scellé. On fait **une
seule** rupture de format, qui ferme d'un coup **deux** bugs qui n'en sont
qu'un :

1. le **comptage des trous** (créneaux jamais écrits lus comme records vivants
   — 10 créés, 132 lisibles [MESURÉ, note PO `3cda8545`]) ;
2. la **fuite trou=public** qu'aurait créée le v1 (`tenant_id=0=public` +
   zéro-remplissage ⇒ tout trou devient visible de tous les tenants).

Le correctif unifiant est le drapeau `ALLOCATED` (§4-J1) **livré dans le même
bump de format** que la colonne `tenant_id` (§4-J2). Jalon 0 (§3) : le bump de
`SUBSTRATE_FORMAT_VERSION` part **dans le même commit** que le layout, avec le
chemin de migration à l'ouverture. Les opérateurs ne vivent **qu'une** fenêtre
de migration.

Fondation vérifiée : le refus de version existe déjà et est propre [VÉRIFIÉ
file.rs:939-944, appelé à l'open :913] — pas de corruption silencieuse
possible entre binaires et stores de versions croisées.

### A3 — Le filtre descend dans les primitives mmap, PAS dans `noeud_visible_pour`

C'est la correction de la décision 2 cassée du v1. Preuves :

- `node_ids` et `nodes_by_label_capped` scannent la zone et filtrent en ligne
  sur `TOMBSTONED` — zéro appel à `noeud_visible_pour` [VÉRIFIÉ
  store.rs:13633-13662, :13674-13721] ;
- un filtre aval (ScanOperator) après `nodes_by_label_capped(max)` rouvrirait
  la **sous-livraison de LIMIT** (la faille du chantier SCAN : tronquer à `max`
  AVANT de filtrer rend moins de k résultats) ;
- `node_count`/`node_count_by_label` sont des atomiques globaux [VÉRIFIÉ
  store.rs:13723, :13733] — un filtre de visibilité ne les corrige pas ; il
  faut des **compteurs par tenant** (§4-J3).

Le tenant se teste donc **dans la même boucle que `TOMBSTONED`**, au même prix
(le record est déjà dans le cache). Les **trois** sites du chemin rapide
`visibilite_triviale` [VÉRIFIÉ store.rs:3787, :13082, :13094] restent
cohérents : ce chemin rapide ne concerne que le MVCC (cartes vides) — le test
tenant est **orthogonal** et s'applique aussi sur le chemin trivial, sans
exception (§4-J3).

### A4 — `Principal { reader, tenant, roles }` : canal typé parallèle, IMMUABLE par session

Correction de la décision 1 trompeuse du v1. `AccessContext` n'est pas le bon
véhicule : il ne porte pas de tenant [VÉRIFIÉ base_registry.rs:246-252], son
`decide` est un prédicat `(reader, slug) → bool` de frontière **inter-bases**
(autre couche), et surtout le champ de session est un
`Mutex<Option<Arc<...>>>` avec un setter public [VÉRIFIÉ session/mod.rs:124,
:362-363] — **mutable**, ce qui contredit l'argument d'immuabilité du v1.

[PROPOSÉ] `Principal { reader: String, tenant: TenantId, roles: Vec<String> }`
est posé **à la construction de la session** et n'a **aucun setter**. Le champ
`access_context` existant n'est ni touché ni réutilisé. Détail en §4-J4.

### A5 — Sentinelle de migration : `u32::MAX` = INVISIBLE, jamais « trou = public »

[PROPOSÉ] Pendant la fenêtre de re-tenanting, un nœud pas encore migré porte
`tenant_id = u32::MAX` : **invisible de tout tenant** (fail-closed). Une
migration interrompue laisse des nœuds invisibles — un incident de
disponibilité, jamais une brèche de confidentialité. `tenant_id = 0` = public
n'est le défaut **que** pour un store qui migre en bloc et **déclare**
mono-tenant public (drapeau posé explicitement dans `substrate.meta` par
l'outil de migration, §5.2 — jamais implicite).

---

## 3. Jalon 0 — Versionnement + migration (la fondation)

**Objectif** : rendre la rupture de format vivable pour les stores déployés.

[PROPOSÉ] Contenu :

- `SUBSTRATE_FORMAT_VERSION: 1 → 2` [meta.rs:8] **dans le même commit** que le
  nouveau layout de `NodeRecord` (§4-J2) — jamais un binaire qui écrit du 40 B
  sous une meta v1.
- **Chemin de migration à l'ouverture** (nouveau module
  `crates/obrain-substrate/src/migration.rs`) : à l'open, si
  `format_version == 1` et que l'appelant a demandé la migration
  (`open_migrating(...)` explicite — l'open normal continue de refuser
  proprement via `UnsupportedVersion` [VÉRIFIÉ file.rs:939-944]) :
  1. **Drainer le WAL v1** : exécuter les passes de fusion existantes
     (`merge_wal_inline_props` [VÉRIFIÉ store.rs:4158],
     `merge_wal_external_edges` [VÉRIFIÉ store.rs:4569]) puis `fsync` et
     tronquer. Nuance au brief : `replay_from()` n'a aucun appelant en
     production [VÉRIFIÉ store.rs:841-844, :877-879] — les zones sont la
     source de vérité, mais le WAL porte des données inline qu'il faut fusionner
     AVANT réécriture, sinon elles seraient rejouées sur un layout 40 B.
  2. Réécrire `substrate.nodes` 32→40 B vers un fichier temporaire
     (`substrate.nodes.v2.tmp`), en posant pour chaque record : `ALLOCATED` si
     le record n'est pas un créneau de padding (l'heuristique actuelle
     [VÉRIFIÉ warden.rs:161-169] sert **une dernière fois**, ici) ; `tenant_id
     = u32::MAX` (sentinelle A5) ou `0` si migration en bloc déclarée (§5.2).
  3. `rename` atomique, puis bump du header meta **en dernier** — c'est le
     point de commit. Kill à n'importe quel instant ⇒ soit un store v1 intact,
     soit un store v2 complet ; jamais un mélange.
- **Idempotence** : relancer `open_migrating` sur un store v2 est un no-op
  (test de version en tête).

**Fichiers touchés** : `meta.rs`, `file.rs` (open/validate), `record.rs`
(layout), nouveau `migration.rs`, `store.rs` (drainage WAL).

**Tests exigés** :
- store v1 + binaire v2 sans migration → `UnsupportedVersion`, message net
  (le test de refus existe déjà pour le cas nominal [VÉRIFIÉ file.rs:1156]) ;
- égalité : dump complet (nœuds, arêtes, props, labels) avant/après migration
  identique ;
- crash : kill -9 à chaque étape (avant rename, après rename avant bump) →
  store réouvrable, jamais mixte ;
- idempotence : double migration = no-op.

**Critère d'acceptation** : aucun chemin de code ne peut lire des records v1
avec un stride v2 ou l'inverse ; la migration d'un store réel (copie d'un store
obrain-tmn) aboutit avec dump identique.

---

## 4. Jalons 1-5

### Jalon 1 — `ALLOCATED` (ferme trou-comptage ET trou-public)

[PROPOSÉ] `pub const ALLOCATED: u16 = 1 << 6;` dans `node_flags` [aujourd'hui
bits 0-5 seuls, VÉRIFIÉ record.rs:61-69]. Sémantique : posé par **tout**
writer de nœud à l'écriture du record, jamais effacé (un tombstone reste
`ALLOCATED | TOMBSTONED`). Un slot à `flags & ALLOCATED == 0` est un trou —
point final, plus d'heuristique.

- Les **deux** sites d'heuristique « tout-à-zéro » [VÉRIFIÉ warden.rs:161-169 ;
  store.rs:9082-9094] basculent sur le test `ALLOCATED`. L'ambiguïté
  `create_node_in_community(&[], 0)` ≡ trou, documentée dans le code même
  [VÉRIFIÉ store.rs:9083-9089], disparaît.
- Les boucles de scan (`node_ids`, `nodes_by_label_capped`,
  `for_each_node_record*`) ignorent les slots non-`ALLOCATED`.

**Fichiers** : `record.rs`, `warden.rs`, `store.rs`. Livré **derrière le même
bump de version** que J2 (arbitrage A2) — pas de format intermédiaire.

**Test d'égalité/isolation exigé** : le scénario mesuré par AGREGATS — N nœuds
créés en communautés clairsemées → exactement N lisibles, N comptés ;
`create_node_in_community(&[], 0)` discernable d'un trou.

**Critère d'acceptation** : `node_ids().len() == node_count()` sur un store
troué ; plus aucune comparaison `flags == 0` dans le substrat.

### Jalon 2 — Colonne `tenant_id` + bump 32→40 B

[PROPOSÉ] Layout `NodeRecord` v2 (40 B, align 8 conservé) :

```text
label_bitset: u64 | first_edge_off: U48 | first_prop_off: U48 |
community_id: u32 | energy: u16 | scar_util_affinity: u16 |
centrality_cached: u16 | flags: u16 | tenant_id: u32 | _reserve: u32
```

`tenant_id` : `0` = public (uniquement déclaré, §5.2) ; `u32::MAX` =
sentinelle non-migré invisible (A5) ; sinon id de tenant interné (§5.1).
`_reserve` maintient taille 40 = multiple de l'alignement 8 et paie d'avance
la prochaine extension.

**Conséquence d'allocation, chiffrée** (adressage linéaire `slot * SIZE`
[VÉRIFIÉ store.rs:8978-8979]) : 40 ∤ 4096, donc l'assertion
`4096/SIZE == NODES_PER_PAGE` [VÉRIFIÉ record.rs:103] doit être **réécrite**,
pas juste recalculée. Deux options :
- `NODES_PER_PAGE = 102` (par page OS de 4 KiB, 16 B de queue morte,
  densité 128→102 soit −20 % ; des records chevauchent les frontières de
  pages OS — inoffensif pour mmap, à re-mesurer pour le prefetch du warden
  [VÉRIFIÉ warden.rs:59]) ;
- **recommandée** : page d'allocation de 20 KiB = 5 pages OS = **512 records
  exactement** (512×40 = 20480), zéro queue morte, maths du warden entières.

Coût brut : +8 B/record (+25 % de la zone nodes). Accepté par l'arbitrage A2
en échange d'une lecture tenant ~ns dans la boucle chaude.

**Fichiers** : `record.rs` (layout + asserts), `warden.rs` (maths de pages),
`store.rs` (allocateur community-local, prefetch, Hilbert/Z-order qui
raisonnent en `NODES_PER_PAGE` [VÉRIFIÉ store.rs:11347-11372]), `migration.rs`.

**Test d'égalité exigé** : suite substrat complète verte sur stores 40 B ;
round-trip Hilbert/Z-order inchangé ; bench de scan avant/après (le record
tient toujours dans 1-2 lignes de cache).

**Critère d'acceptation** : store migré = dump identique + `tenant_id`
sentinelle partout ; asserts de compilation à jour ; aucun code ne suppose 32.

### Jalon 3 — Filtre tenant dans les primitives + compteurs par tenant

[PROPOSÉ] Le cœur de l'arbitrage A3.

- Les primitives de scan prennent une portée :
  `TenantScope::Un(TenantId) | Public | Systeme`. `Systeme` est réservé aux
  chemins internes (warden, compaction, backup, migration) et **toujours
  explicite** — jamais une valeur par défaut.
- Dans `node_ids` [store.rs:13633] et `nodes_by_label_capped` [store.rs:13674],
  le test s'ajoute **dans la même boucle** que `TOMBSTONED` (et `ALLOCATED`,
  J1) : `rec.tenant_id == scope || rec.tenant_id == 0` pour `Un(t)` — le
  public reste lisible de tous ; `u32::MAX` n'égale jamais un scope. La borne
  `max` de `nodes_by_label_capped` compte **après** filtre : LIMIT k rend k
  (la sous-livraison SCAN ne rouvre pas).
- Cascade arêtes (A1) : `edges_from` [store.rs:13481] vérifie
  `tenant_visible(far_endpoint)` avant de pousser la paire — une lecture de
  NodeRecord par arête, colonne ~ns. Les deux sens (`walk_outgoing_chain`,
  `walk_incoming_chain`) — le sens entrant est celui que this-rs utilise
  réellement (`find_by_target`, constat de la revue).
- **Compteurs par tenant** : `node_count`/`node_count_by_label` globaux
  [VÉRIFIÉ store.rs:13723, :13733] doublés d'une matrice
  `tenant × (1 + 64 labels)` d'atomiques (≈ 520 B par tenant, dimensionnée par
  le dictionnaire §5.1), maintenue aux mêmes points que `total_live_nodes` et
  `label_live_counts`. Les compteurs globaux restent pour `Systeme`.
- **Cohérence des trois sites `visibilite_triviale`** [store.rs:3787, :13082,
  :13094] : le chemin rapide MVCC continue de court-circuiter les cartes de
  transactions, **mais la vivacité qu'il teste (`is_live_on_disk` /
  `filtrer_noeuds_vivants`) devient « vivant ET dans la portée tenant »**. Le
  tenant n'a pas de chemin rapide : il se teste toujours, aux trois sites,
  par le même prédicat partagé.

**Fichiers** : `store.rs` (primitives, compteurs, cascade), trait de scan côté
`obrain-core`/`obrain-engine` (propagation de la portée), ScanOperator.

**Tests d'égalité/isolation exigés** :
- égalité : store déclaré mono-tenant public → **toute la suite existante
  verte, résultats identiques** à v1 ;
- isolation : deux tenants A/B mélangés → `MATCH (n) RETURN n LIMIT k` sous A
  rend k nœuds de A ; compteurs par tenant exacts ; les trous (J1) et les
  sentinelles `u32::MAX` invisibles des deux ;
- cascade : arête A→B invisible de A et de B ; nœud `_link` de A invisible
  de B ;
- MVCC : les trois sites rendent le même verdict pour un même (nœud, portée),
  cartes vides ou non.

**Critère d'acceptation** : aucun chemin de lecture de nœud n'atteint la mmap
sans passer le prédicat tenant ; bench scan mono-tenant dans le bruit de v1.

### Jalon 4 — `Principal` : canal typé immuable par session

[PROPOSÉ] L'arbitrage A4.

- `Principal { reader: String, tenant: TenantId, roles: Vec<String> }` dans
  `obrain-core` ; la session l'accepte **à la construction**
  (`Database::session_as(principal)`) et le stocke en champ nu (pas de
  `Mutex`, pas d'`Option` mutable, **aucun setter**) — par contraste vérifié
  avec `access_context` [VÉRIFIÉ session/mod.rs:124, :362-363], qui reste en
  place pour la frontière inter-bases et n'est pas touché.
- La session dérive `TenantScope` du `Principal` et le passe aux primitives
  (J3). `roles` est transporté, **pas interprété** par le substrat (§6).
- Sans `Principal` : store v2 multi-tenant → portée vide, **rien** n'est rendu
  (fail-closed, cohérent avec la décision viz §5.3) ; store v2 déclaré
  mono-tenant public → `Public` implicite (c'est ce qui garde `obrain-cli`
  et les REPL utilisables sur les stores locaux).

**Fichiers** : `obrain-core` (type), `obrain-engine/src/session/mod.rs`
(construction, exécution), `obrain-cli` (points d'entrée).

**Test exigé** : à la compilation, `Principal` n'expose aucune mutation ;
à l'exécution, deux sessions concurrentes de tenants différents sur le même
`Arc<ObrainDB>` sont étanches (le test d'isolation J3 rejoué par sessions).

**Critère d'acceptation** : aucun `set_principal` dans l'API publique ; la viz
et le bridge MCP compilent en fournissant un principal ou en assumant la
portée vide.

### Jalon 5 — Consommateurs

Le contenu est la section §5 ; le jalon consiste à l'exécuter dans l'ordre :
mitigation viz (immédiate, avant tout code), re-pointage obrain-tmn,
migration this-rs pilotée, puis bascule des trois sur le binaire v2.

**Critère d'acceptation global** : les trois consommateurs tournent sur des
stores v2 ; zéro nœud en sentinelle `u32::MAX` à la fin de chaque fenêtre ;
le port 8432 ne sert plus rien sans principal.

---

## 5. Mise à jour propre des consommateurs déployés

### 5.1 this-rs — le seul à connaître ses tenants, donc le pilote

**Point d'entrée** [VÉRIFIÉ] : `ObrainStore::open` → `ObrainDB::open`
[obrain.rs:130-131] (+ `open_read_only` :151-153) ; identité JWT
`StsClaims { sub, tenant_id: Uuid, roles }` [sts.rs:16-22] ; liens = nœuds
`_link:{type}` [obrain.rs:77] ; dépendances par chemin vers
`../../banc-obrain/crates/*` [this/Cargo.toml:98-116].

**Internement Uuid → u32** [PROPOSÉ] : le dictionnaire appartient **au store**,
pas à this-rs — recommandation confirmée par le terrain : `substrate.meta` est
« 4 KiB file header + dictionary section » [VÉRIFIÉ meta.rs:1-2, header 96 B
:20-38] et le substrat a déjà le motif du catalogue plat texte réparable
(`substrate.indexcat` [VÉRIFIÉ store.rs:870], `substrate.proccat`).
Concrètement : un `substrate.tenantcat` (une ligne `uuid \t u32 \t
libellé`), relu à l'ouverture, append-only, u32 jamais réutilisé. Le store qui
porte les données porte la table qui les interprète — un store copié reste
interprétable seul.

**Chemin de migration** [PROPOSÉ] :
1. mettre `~/projects/this-rs` **sous git** (constat : pas un dépôt — vérifié) ;
2. arrêt du service ; `open_migrating` (§3) → tout nœud à `u32::MAX`,
   invisible ;
3. this-rs re-tenante par lots : pour chaque entité et chaque nœud
   `_link:{type}`, il résout le tenant propriétaire (lui seul le connaît),
   interne l'Uuid via le dictionnaire, écrit `tenant_id`. Une interruption
   laisse le reste **invisible** (A5) — on relance, jamais de fuite ;
4. fin de fenêtre : assertion `zéro nœud à u32::MAX` (requête `Systeme`),
   puis service relancé — chaque requête construit
   `Principal { reader: sub, tenant: interné(tenant_id), roles }` à la session
   (J4).

**Critère** : les tests existants de this-rs (reopen, links, search survivent)
verts sur store v2 ; un JWT du tenant A ne peut lire aucune donnée de B, même
via les chemins `find_by_target`.

### 5.2 obrain-tmn — mono-tenant, migration en bloc

**Point d'entrée** [VÉRIFIÉ] : `ObrainDB::open_substrate`
[obrain_db_pool.rs:569, :1271], `SubstrateStore::open`
[exocortex_bridge.rs:1292].

**Préalable bloquant** [VÉRIFIÉ Cargo.toml:13-21] : ses dépendances pointent
vers `~/projects/lab/grafeo-pillars`, **qui n'existe pas** sur ffs-n3. Le
binaire déployé est orphelin de ses sources de dépendances. Étape 1 :
re-pointer vers `~/projects/banc-obrain/crates/*` (ou un pin vendorisé) et
reconstruire — sans ça, « migrer obrain-tmn » n'a pas de sens.

**Chemin de migration** [PROPOSÉ] : ses stores sont mono-exploitant. Un
binaire v2 les **refuse proprement** tant qu'ils sont v1 [VÉRIFIÉ
file.rs:939-944] — comportement voulu : rien ne casse silencieusement, le pool
remonte l'erreur `UnsupportedVersion`. Migration triviale : `open_migrating`
en mode **bloc déclaré** → tout nœud à `tenant_id = 0`, drapeau
`mono_tenant_public` posé dans la meta par l'outil (A5 : le public se déclare,
ne se subit pas). Sessions sans principal → portée `Public` (J4), zéro
changement de code applicatif au-delà du re-pointage.

**Critère** : chaque store du pool migré en bloc ; `obrain info` affiche
version 2 + drapeau ; les huit index recréés en dur à l'ouverture
[VÉRIFIÉ store.rs:845-848, commentaire] fonctionnent à l'identique.

### 5.3 viz :8432 — ensemble vide, jamais fallback public

**Point d'entrée** [VÉRIFIÉ] : `obrain viz serve`, port 8432 par défaut
[commands/viz.rs:207], handles RO partagés sans identité
[viz_serve.rs:31-44], bind localhost mais exposable par `--host`
[viz_serve.rs:10].

**Mitigation immédiate (avant tout jalon)** : fermer/ne plus exposer le port
(pare-feu, suppression de `--host`) — déjà identifiée, indépendante du code.

**Décision** [PROPOSÉ] : la viz ne reçoit pas de principal en v2 → sur un
store multi-tenant elle obtient la **portée vide** : sous-graphe vide, pas
d'erreur, **jamais** de repli sur `Public` des données d'autrui (seuls les
nœuds `tenant_id = 0` d'un store déclaré public restent rendables). Le jour où
la viz doit montrer un tenant, elle le fera en construisant un `Principal`
authentifié — hors de ce RFC.

**Critère** : sur un store de test à deux tenants, `GET /api/viz/subgraph`
sans principal rend un graphe vide ; sur un store obrain-tmn public, la viz
est inchangée.

---

## 6. Ce que ce RFC ne fait pas

- **RBAC / autorisation d'opération** : `Principal.roles` est transporté,
  jamais interprété par le substrat. Décider qui peut ÉCRIRE quoi reste à la
  couche au-dessus (obrain-iam, this-rs `Acl`). Ce RFC ne traite que la
  **confidentialité de lecture inter-tenant**.
- **Tenant d'arête** : le `_pad: [u8; 2]` d'`EdgeRecord` [VÉRIFIÉ
  record.rs:306] est réservé, pas câblé (A1). La métadonnée d'un lien entre
  deux nœuds visibles n'est pas confinable indépendamment — M2+.
- **Isolation intra-tenant** : deux utilisateurs d'un même tenant voient la
  même chose au niveau substrat ; les ACL fines restent applicatives.
- **Le trou MVCC des arêtes** : pas de carte `en_attente_aretes` [VÉRIFIÉ
  store.rs:13074-13078] — trou d'isolation transactionnelle connu, orthogonal
  au tenant, à traiter par la méthode campagne (le test d'abord).
- **Chiffrement, quotas, facturation par tenant** : hors sujet.

---

## Annexe — corrections au dossier de revue (dérives constatées aujourd'hui)

| Fait | Cité par la revue | Constaté @ afbb5cfc |
|---|---|---|
| Sites du chemin rapide | store.rs:3389/12531/12538 | store.rs:3787/13082/13094 |
| 2e site heuristique trous | store.rs:8929-8935 | store.rs:9082-9094 |
| `edges_from` | store.rs:12931 | store.rs:13481 |
| « migration passe le WAL » | tel quel | à préciser : `replay_from` sans appelant prod [store.rs:841-844] ; drainer `merge_wal_inline_props`/`merge_wal_external_edges` puis tronquer |
| obrain-tmn constructible | implicite | dépendances vers chemin inexistant [Cargo.toml:13-21] |
| this-rs versionné | implicite | pas un dépôt git sur ffs-n3 |
