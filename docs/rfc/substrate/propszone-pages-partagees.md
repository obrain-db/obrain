# RFC — PropsZone v2 : unité de page 512 o (slots) pour les chaînages courts (densité + coût d'append)

> **Statut** : RFC à valider — **aucun code écrit**. Suite du chantier « double
> journalisation WAL des propriétés » (option C livrée `ed8e5bd5`/`8d710b99`,
> statut LIVRÉE `25dc65c2`). Décidée avec l'utilisateur (« on fera 1 ») :
> attaquer le poste structurel restant du chemin d'écriture des propriétés —
> la **granularité 4K de la zone** — pour les workloads à nœuds frais + props
> (CREATE GQL, imports).
>
> **Date** : 2026-09-06 — branche `feat/zorder` — HEAD `25dc65c2`.

---

## 1. Le problème mesuré

Deux coûts structurels liés à la granularité 4K de `substrate.props.v2` :

1. **Une page 4K dédiée par 1ʳᵉ prop de chaque nœud/arête frais**
   (`allouer_page_neuve`, props_zone.rs:503-526 : toute chaîne démarre sur une
   page neuve à `owner_id` unique). Une page = header 24 o + payload 4072 o ;
   une entrée scalaire = 4-12 o → **~339 entrées possibles par page, on en met
   une**. Mesure (sonde `waste`, mono, release, 2026-09-06) : **5 000 nœuds +
   1 prop chacun → `substrate.props.v2` = 22 093 824 o (5 394 pages 4K) pour
   ~100 Ko utiles → remplissage 0,45 %**. Soit ~4,4 Ko d'espace + ~2 µs
   (allocation/écriture 4K) par nœud frais.
2. **Chaque append est un read-modify-write COMPLET de la page 4K** : lecture
   4096 o (`read_page`), append au `free_offset`, re-`seal_crc32()` sur les
   4096 o (page.rs:163-180), réécriture 4096 o (`write_page`,
   props_zone.rs:359-366) — même pour une entrée de 4 o. C'est le plancher du
   cas « chaude » (2,03 → 0,80 µs après option C, dont cette mécanique).

**Impact** : CREATE (nœud frais + 1ʳᵉ prop) ≈ **7,4 µs/op** mono (mesure
perf_prop_wal/multi-graphes) dont ~2 µs d'allocation 4K + ~2 µs de WAL (2
syscalls, structurels) ; imports bulk avec propriétés → zones
multi-gigaoctets à remplissage <1 %. Le cas « chaude » (props sur nœuds
existants) est déjà bon (0,8 µs) mais paie toujours le RMW 4K par append.

## 2. Layout actuel (faits durs, HEAD `25dc65c2`)

- `PAGE_SIZE = 4096` (page.rs:63) ; `PropertyPageHeader` 24 o sans padding :
  `magic u32` (node `0xF5075FA6` / edge `0xE5075FA6`), `node_id u32` (=
  **owner id, unique par page**), `crc32 u32`, `next_page U48`, `entry_count
  u16`, `free_offset u16`, `tombstones u16` (page.rs:70-82) ; payload 4072 o.
- Entrées inline `[flags u8][prop_key u16][tag u8][payload]` (page.rs:232-256),
  taille min 4 o (I64 = 12 o), append séquentiel au `free_offset`
  (page.rs:531-548) ; `ENTRY_FLAG_TOMBSTONE 0x01`.
- Chaînage **newest → oldest** par page (`next_page`) ; la tête
  `first_prop_off` (U48, octet 14 du NodeRecord / 24 de l'EdgeRecord)
  n'encode **qu'un index de page u32** (`encode_page_id`/`decode_page_id`,
  props_zone.rs:97-105) — **16 bits hauts de l'U48 libres**.
- Deux fichiers mmap : `substrate.props.v2` + `substrate.props.heap.v2`
  (chaînes) ; croissance ×1,5 (remap `set_len`), shrink du padding par compact
  CLI ; page 0 = sentinelle ; frontière live/padding par **recherche binaire
  sur le magic** (props_zone.rs:143-194) — tout nouveau type de page doit
  avoir un magic reconnu par ce balayage.
- Lecture : les walkers (`walk_chain` 571-601, `collect_live_props_by_key`
  632-679, `get_latest_for_key` 704-739, `lookup_*_v2` store.rs:7366-7405)
  vérifient **le magic seulement** — jamais l'owner, jamais le CRC en
  production (CRC vérifié en test seulement, page.rs:663-672).
- Pas de compaction de chaînes ni de pages (champ `tombstones` écrit, jamais
  lu en prod ; « compaction deferred », props_zone.rs:745 ; seule opération =
  shrink du padding). `SUBSTRATE_FORMAT_VERSION = 1` (meta.rs:8).

## 3. Pourquoi c'est un chantier de FORMAT (la décision de gouvernance)

Contrairement au WAL (un kind inconnu arrête proprement le rejeu d'un vieux
binaire — wal.rs:403-406), la **zone ne tolère pas un magic de page inconnu** :
`compute_next_page_idx` (props_zone.rs:165-194) compte tout magic inconnu
comme padding → un vieux binaire ouvrant une zone au nouveau format
**réallouerait par-dessus les nouvelles unités et décoderait des entrées
d'autrui** (corruption silencieuse). Conséquences :

- Tout nouveau type d'unité (page ou slot) impose **`SUBSTRATE_FORMAT_VERSION`
  bumpé + étape de migration `obrain-migrate`** (règle lib.rs:25-27,
  format-spec.md:264), OU une **garde stricte « unités neuves uniquement sur
  zones créées par le nouveau binaire »** avec refus d'ouverture des vieux
  binaires.
- Coût : les ~1 528 bases v2 héritées (marqueur absent) et les bases v2
  existantes ne passent au nouveau format que par migration ; les bases neuves
  naissent directement au nouveau format (pattern `a4c1edaf`/`finalize-v2`).
- L'**audit d'ancres post-rejeu** (replay.rs:234-323) suppose « ancre = index
  < frontière de pages » ; une ancre (page, slot) exige une règle de validité
  étendue. `decode_page_id` caste muet u32 → à sécuriser.

## 4. Cartographie (faite — synthèse du rapport de lecture)

- **Append** : `set_node_property` → `append_scalar_to_props_zone_v2`
  (store.rs:7108-7178) → intern clé, `read_node` (tête), `pz.append_entry` sous
  verrou écrit. 1ʳᵉ prop → `allouer_page_neuve(owner, kind, None, entry)`
  (props_zone.rs:495) ; append page existante → RMW 4K + re-seal + `write_page`
  (481-483) ; page pleine → nouvelle tête chaînée (`next_page`). Le changement
  de tête est journalisé **WAL-first** (`NodePropHeadUpdate`, writer.rs:341-368,
  rejoué replay.rs:1229-1248) ; le CONTENU des entrées n'est pas journalisé
  sous v2 (option C) — durabilité = mmap + msync au flush
  (`PropsZone::flush`, props_zone.rs:794-800).
- **Lecteurs** : walkers + store (`lookup_*_v2`, `read_all_properties_for_*`
  10917-11060 en fusion 4 passes), relevés slot→head 1:1 et sweeps index.
  Hypothèse « une page = un propriétaire » aux 5 sites : garde d'append
  (418-477), walkers (571-739, aucune entrée ne porte d'owner), lectures par
  tête de record, relevé 1:1 (1661-1698), audit d'ancres (replay.rs:234-323).
- **`slot_remap`** (HilbertRepermuteV2 / CompactCommunityV2) **ne touche pas
  aux pages de props ni à leurs owner stamps** (zone non enrôlée,
  store.rs:4623-4646 ; records déplacés entiers, writer.rs:1605-1612).
  **Risque latent préexistant découvert** : après une permutation, le record
  (et sa tête) est au nouveau slot mais les pages portent l'owner du vieux
  slot → la garde d'append (props_zone.rs:459-462) juge la tête « d'un autre
  propriétaire » et **détache la chaîne au prochain append** (nouvelle page
  sans `next_page` → l'ancienne chaîne sort du walk). Les lectures, elles,
  restent correctes. Le nouveau format devra rendre les owner stamps
  remap-aware (ou adresser autrement) — et ce risque mérite un correctif
  séparé avant/indépendamment de cette RFC.
- **Tests qui figent le layout** (DoD) : unitaires page.rs:606-926 /
  props_zone.rs:819-1425 (dont `multiple_entries_fit_on_one_page` 845,
  `overflow_triggers_chain_expansion` 863, `round_trip_10k_nodes_mixed_props`
  941, LWW 1014-1097, tombstones 1124, edge chains 1161-1270) ;
  writer.rs:3735-3891 (têtes @octet 14/24) ; intégration store 21082-21788
  (dont `props_zone_v2_survives_reopen` 21246 qui suit `next_page` à la main) ;
  kill/crash `*_regime_v2` (20548-20918 + preuves option C) ; migration /
  finalize / drains (21855-25585) ; zone/shrink wal_checkpoint.rs:153-180 ;
  churn (`perf_churn` — lecture O(réécritures)).

## 5. Options

- **A. Pages partagées (entrées multi-propriétaires).** L'entrée porte l'owner
  (+ lien intra-page), la tête encode (page, offset). **Cassant** (v3) : codecs
  d'entrée (ENTRY_HEADER_SIZE 4→6/8), toute la couche de lecture devient
  owner-aware, LWW par (owner, clé) plus coûteux, et le **RMW 4K + CRC reste
  inchangé**. Avantage faible vs coût → écartée en première intention.
- **B. Unités de 512 o (7 slots utiles par page 4K) — RECOMMANDÉE.** Chaque
  slot = « page logique » d'un propriétaire (header/CRC par slot de 512 o ; le
  slot conserve owner_id/magic/next_page). L'unité 0 du 4K sert d'en-tête de
  carrier (magic packé + nb d'unités utilisées) — 8 × 512 = 4096 exactement,
  donc 7 slots par carrier. La tête U48 encode (carrier, slot) via les 16 bits
  hauts libres (discriminateur : bit 47 = 1 = slot), sans élargir les records.
  **Additif** : les têtes v2 (bits hauts 0) restent lisibles ; un slot se
  présente aux walkers comme une page → invariants d'owner, LWW et tombstones
  **inchangés**, coût d'append (RMW + CRC) divisé par ~8, 7 petits chaînages
  par page 4K (densité ×7 sur le workload CREATE, remplissage ~0,45 → ~3 %).
  Limites : granularité fixe 512 o (un chaînage de 600 o consomme 2 slots),
  saut de slot dès 7 chaînages par page ; le codec des têtes, l'audit d'ancres
  et la frontière (unité slot) changent ; les owner stamps restent à rendre
  remap-aware (cf. §4). Repli des vieilles bases : migration des têtes v2→v3
  (pattern finalize), ou garde zones-neuves + refus vieux binaires.
- **C. Ne rien faire / documenter.** Le cas « chaude » est déjà à 0,8 µs ;
  les workloads CREATE restent à ~7,4 µs et les imports gonflent les zones.
- **Sous-option (mesurable avant tout format)** : réduire le RMW/CRC à la
  portion `[0..=free_offset]` au lieu des 4096 o — le CRC n'est pas vérifié en
  production aujourd'hui (walkers), mais changer sa portée modifie le format
  des pages existantes : à chiffrer d'abord sur synthétique (A/B 512 vs 4096)
  avant tout engagement.

> **ÉTAPE 0 (mesurée, 2026-09-06, mono, release — micro `gran512`) :** le RMW
> d'append est dominé par la copie 2×N + le checksum N. Unité **4096 o** :
> memcpy 2×N ~0,18 µs + checksum-proxy 3,88 µs (FNV logiciel — borne haute :
> `crc32fast` SIMD réel est ~10× plus rapide, ~0,2-0,4 µs/4K). Unité **512 o** :
> memcpy 2×N ~0,02 µs + checksum-proxy 0,47 µs. Soit un **rapport ~8× sur le
> poste RMW** par append. Sur le chemin réel (chaude 0,8 µs après option C,
> dont la mécanique RMW 4K), le passage en unité 512 retrancherait ~0,2-0,4 µs
> par append en régime établi, et ~1,5 µs + ~4 Ko/espace par 1ʳᵉ prop (page
> 512 au lieu de 4K) sur le workload CREATE. Le gain d'espace (0,45 % → ~4 % de
> remplissage sur le motif CREATE) reste le bénéfice dominant pour les imports.
> A/B réel (PropsZone 512 vs 4096) à confirmer dans le DoD avant tout
> engagement de format.

**Recommandation** : B, précédée d'une **preuve de rentabilité** (étape 0) —
mesurer sur synthétique le gain RMW/CRC 512 o vs 4096 o et la réduction
d'espace réelle, AVANT d'ouvrir le chantier de format ; et traiter le risque
remap (correctif séparé) en parallèle.

## 6. DoD (provisoire — à geler avec le design retenu)

1. RFC validée ; étape 0 (preuve de rentabilité A/B 512 vs 4096 sur
   synthétique) datée ; option confirmée.
2. Décision de gouvernance prise : bump `SUBSTRATE_FORMAT_VERSION` + migration
   `obrain-migrate`, OU garde « zones neuves seulement » + refus vieux binaires
   (le chantier exige l'un des deux, §3).
3. Correctif remap (owner stamps) livré ou explicitement ordonné avant/avec.
4. Preuves de non-perte : crash/reopen et kill sans flush sur zones mixtes
   (unités v2 + slots), rejeu WAL, migration de têtes, audit d'ancres étendu.
5. Toutes suites vertes : substrate lib, engine lib, core, invariants,
   t17k v2, migration/finalize, kill `*_regime_v2`, churn ; + nouveaux tests
   slots (page.rs, props_zone.rs, store, audit).
6. Bench daté avant/après : CREATE mono/@8/@16, chaude, import bulk ;
   sonde `waste` (remplissage %) et `perf_prop_wal` ; WAL inchangé.
7. Ledger + commit(s) réversibles, 0 rouge.

## 7. Migration native — EXIGENCE (bases harness en priorité)

Décision utilisateur : la bascule des bases existantes vers le nouveau format
se fera **nativement et sans erreur**, avec une attention particulière aux
**bases des harness** (fleet grafeo/obrain : `agent`, `code-all3`,
`dsh-runtime`, bases bridge/cbm, etc.). Traduction dans le chantier :

1. **Migration = première classe, pas un appendice** : étape `obrain-migrate`
   native (pattern `--finalize-v2` déjà éprouvé : snapshot préalable, drain
   idempotent et reprisable, relecture de contrôle, suppression du sidecar
   seulement après succès — store.rs `finalize_props_v2` 6624, `delete_legacy_
   props_sidecar` 6120, tests 21855-25585). Pour le format slots : migration
   des TÊTES (v2 → slot) + marqueur, drain non destructif, reprise après
   interruption (pattern `base_a_migration_interrompue`, 25248+).
2. **Garde-fous « sans erreur »** : dry-run ; relecture-échantillon +
   comptages (parité counts/valeurs avant suppression de l'ancien état) ;
   opération par copie (tmp+rename) ; tout échec laisse la base lisible dans
   l'ancien format (jamais d'état hybride illisible) ; 0 perte sur kill/crash
   en cours de migration.
3. **Portes de non-régression harness** : QC automatique sur au moins une base
   harness réelle (ex. `code-all3` 445 493 nœuds / 1 670 005 arêtes, contrôle
   18 points utilisé par la bascule T17) avant ET après migration — comptes,
   valeurs échantillonnées, fenêtres exact==sorted, lookups indexés. Aucun
   déploiement sans cette porte verte.
4. La forme retenue (§5-B) minimise la surface de migration (têtes seulement,
   contenu des pages intact) — c'est un argument de sélection supplémentaire
   de B sur A (A réécrit toutes les entrées).

## 8. Contexte tissé dans le graphe

Symboles concernés (CONCERNS à créer) : `PropsZone` (`append_entry_inner`,
`allouer_page_neuve`, `walk_chain`, `get_latest_for_key`,
`collect_live_props_by_key`, `compute_next_page_idx`, `encode_page_id`),
`PropertyPage` (`page.rs`), `SubstrateStore` (`append_scalar_to_props_zone_v2`,
`lookup_node_property_v2`, `read_all_properties_for_node`), `slot_remap`,
audit d'ancres (`replay.rs`). Clé : **`propszone-slots-512`**.
