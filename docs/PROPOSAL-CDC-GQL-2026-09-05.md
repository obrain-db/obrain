# Proposition — chantier « CDC × chemin d'écriture GQL » (boucle mémoire de l'agent)

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD `db8acac8`.
**Statut** : EXÉCUTÉ (round CDC×GQL, commit du chantier) — observer substrate + adaptateur engine (cfg cdc) + anti-doublon crud ; tests engine 2/2 ; porte E2E writer import_events → search_surprise = 6 lignes (avant : 0).
**But** : que `search_delta` / `search_surprise` / `search_conflicts` voient les
mutations écrites par le chemin RÉEL du runtime (GQL via `session.execute` —
`import_events` du bridge, `merge_node`/`link_edge`), pas seulement celles de
l'API `crud`. Constat prouvé en direct : sur `runtime-dev.obrain`,
`CALL obrain.search_surprise(2 h)` → vide malgré l'activité de la session.

## 1. Constat code

- Le CDC est enregistré dans **l'API moteur** (`crates/obrain-engine/src/database/crud.rs`
  : `record_create_node`/`record_…`, appelés par `ObrainDB::create_node`/`create_edge`/
  `set_node_property`) → `CdcLog::record`.
- Le chemin **GQL/executor** écrit via les opérateurs de mutation
  (`crates/obrain-core/src/execution/operators/merge.rs` `MergeOperator`,
  `MergeEdgeOperator` → `store.create_edge_with_props`/`create_node_with_props`
  sur `SubstrateStore`) — **aucun enregistrement CDC** (le câblage de `c935b9d1`
  ne portait que la LECTURE `CALL search_delta`).

## 2. Design retenu (v1) — observateur de mutations substrate, dédupliqué

- **Côté substrate** (`crates/obrain-substrate/src/store.rs`) : champ optionnel
  `mutation_observer: RwLock<Option<Arc<dyn StoreMutationObserver>>>` +
  setter public. Trait minimal (nouveau module `mutation_observer.rs`) :
  ```rust
  pub trait StoreMutationObserver: Send + Sync {
      fn node_created(&self, id: NodeId, epoch: u64);
      fn node_deleted(&self, id: NodeId, epoch: u64);          // v1.1
      fn edge_created(&self, id: EdgeId, src: NodeId, dst: NodeId, epoch: u64);
      fn edge_deleted(&self, id: EdgeId, epoch: u64);          // v1.1
  }
  ```
  Appels au point unique : fin de `SubstrateStore::create_node` et
  `create_edge` (un seul `load` atomique quand absent → coût ~0).
  Ne PAS brancher les chemins bulk `no_wal`/zone-direct (non journalisés CDC par
  conception) — documenté.
- **Côté engine** (`cfg feature = "cdc"`) : adaptateur
  `CdcMutationAdapter { cdc_log: Arc<CdcLog> }` installé dans
  `with_substrate_store` → `ChangeEvent { entity_id, kind: Create, epoch:
  EpochId(epoch du store), timestamp: now_ms, before: None, after: None }`
  → `cdc_log.record`.
- **Anti-doublon** : quand l'observateur est installé, les enregistrements
  node/edge-CREATE de `crud.rs` sont neutralisés par un drapeau base
  (`cdc_observer_active`) — les écritures API (`db.create_node`…) passent alors
  par l'observateur (1 seul événement), et l'enregistrement des PROPRIÉTÉS
  (updates, plus riches : before/after) reste dans `crud` (inchangé, pas de
  double). Tests engine existants (substrate, `search_delta`…) à rejouer pour
  vérifier l'invariance (≥5 événements, kinds create/update, read-only).
- **Périmètre v1** : CREATE node/edge. Les DELETES et SET de propriétés via GQL
  restent non couverts dans v1 (documenté, delta/surprise fonctionnent déjà sur
  les créations — le signal d'activité principal du runtime).

## 3. DoD (8 cases)

1. Observateur substrate + setter + appels en `create_node`/`create_edge`,
   testé (unité : hook reçoit id/src/dst/epoch exacts ; absent = no-op).
2. Adaptateur engine cfg cdc + installation dans `with_substrate_store`
   (+ drapeau anti-doublon crud) ; testé.
3. Tests engine (feature cdc) : (a) GQL `CREATE (:A {..})` ×2 puis
   `MERGE (a)-[:T]->(b)` via `session.execute` → `search_delta(0)` contient ces
   3 créations (nœuds + arête, kind Create) ; (b) API `db.create_node` sur base
   substrate → exactement 1 événement (pas de doublon crud+hook) ;
   (c) `search_surprise(since)` non vide sur ce scénario ;
   (d) portes : engine lib 680 (cdc,algos 724), core 870, substrate ≥804.
4. **Porte « ma session »** (manuelle, RO) : `CALL obrain.search_surprise`/
   `search_delta` sur une copie récente de `runtime-dev.obrain` → NON vide.
5. Bench : surcoût observateur absent ≈ 0 (load atomique) ; présent : mesuré sur
   import bulk (µs/op), borné (1 dynamic-dispatch par création).
6. Commit réversible + chiffres datés dans le ledger ; aucun déploiement prod.
7. Doc : cette proposition + note dans le ledger.
8. Aucun script JS pour la logique (le chantier est 100 % Rust).

## 4. Alternatives écartées (documentées)

- Observateur dans les opérateurs core (merge.rs) : injecté par plan → risque
  de trous (opérateur CREATE distinct non couvert) et plomberie cross-crate ;
  le substrate est le point où TOUTES les écritures passent (une seule
  implémentation).
- Enregistrer dans `crud` seulement en désactivant GQL : impossible (le runtime
  écrit en GQL).
- Double enregistrement systématique : rejeté (pollution du journal).

## 5. Risques

- R1 doublon crud/hook → couvert par la porte (b) + drapeau.
- R2 observateur ajouté à chaud sur `create_edge` (chemin chaud bulk) → coût
  borné et mesuré (DoD 5) ; absent par défaut (store sans observateur = 0 coût).
- R3 chemin bulk `no_wal`/zone-direct hors CDC → documenté (pas un bug, un choix).
- R4 feature-gating : adaptateur et drapeau sous `cfg(feature="cdc")` — aucun
  changement sur les builds sans cdc.
