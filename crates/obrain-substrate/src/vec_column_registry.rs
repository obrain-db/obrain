//! `VecColumnRegistry` — runtime routing of `Value::Vector` properties
//! to the dense mmap vec-column zones defined in [`crate::vec_column`].
//!
//! # Role in the stack
//!
//! ```text
//! SubstrateStore::set_node_property(id, key, Value::Vector(v))
//!     └──► VecColumnRegistry::write_node(id, key, v)
//!            └──► get-or-create VecColumnWriter for (key, Node, dim)
//!                   └──► write_slot(id as u32, cast_slice(&v))
//! ```
//!
//! A `VecColumnRegistry` is the sole gatekeeper of the
//! `substrate.veccol.*` zone family. No other code in the crate
//! opens those files directly — going through the registry ensures:
//!
//! 1. **Spec uniqueness per (key, entity)** — the first `Value::Vector`
//!    write for a given property key establishes its dim + dtype;
//!    subsequent writes with a mismatching dim are rejected rather
//!    than silently corrupting the zone.
//! 2. **Writer reuse** — we keep one open writer per `VecColSpec`
//!    for the lifetime of the store, so a migration loop of
//!    "`set_node_property` per node" does not pay per-write
//!    `mmap` or `fsync` costs. Durability is batched to
//!    [`SubstrateStore::flush`](crate::store::SubstrateStore::flush).
//! 3. **Read consistency** — reads issued in the same session
//!    after a `set_node_property` see the most recent value, even
//!    though the on-disk header hasn't been `sync`'d yet. The
//!    writer serves reads from its own mmap (see
//!    [`VecColumnWriter::read_slot`]).
//!
//! # Crash safety
//!
//! Writes are **not** individually durable. The contract is:
//!
//! - Between two `SubstrateStore::flush` calls, all writes are
//!   visible in-process but the on-disk header still carries the
//!   stale CRC from the last `sync`.
//! - `flush` iterates every open writer and calls `sync` on it:
//!   CRC is recomputed, header is overwritten, the zone is
//!   `msync`+`fsync`'d. Post-`flush`, the zone is durable.
//! - A crash between two flushes loses the writes back to the last
//!   `sync`-point; the next open validates the CRC and, on
//!   mismatch, the reader returns `None`, which the store
//!   translates to "property absent".
//!
//! This is the same durability model as the bincode sidecar it
//! replaces. Tightening crash-safety (per-write WAL) is a T17
//! concern, not T16.7.

use std::sync::Arc;

use dashmap::DashMap;
use obrain_common::types::PropertyKey;
use parking_lot::RwLock;

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::SubstrateFile;
use crate::vec_column::{EntityKind, VecColSpec, VecColumnWriter, VecDType};

/// Routing table + open-writer cache for vector-typed properties.
///
/// Cloneable via `Arc`-internal DashMaps, so the `SubstrateStore`
/// can share the registry across its various method paths without
/// refactoring the store's ownership model.
pub(crate) struct VecColumnRegistry {
    /// `(PropertyKey, EntityKind) -> VecColSpec`. Populated on every
    /// first write of a vector for a given key, and pre-populated on
    /// open from the persisted `DictSnapshot.vec_columns` list plus
    /// the prop-key names (so reads on a freshly-opened store can
    /// find the right spec from the property-key alone).
    by_key: DashMap<(PropertyKey, EntityKind), VecColSpec>,
    /// `VecColSpec -> Arc<RwLock<VecColumnWriter>>`.
    ///
    /// (2026-08-08) Mutex → RwLock — même correctif que
    /// `BlobColumnRegistry::writers` (voir son commentaire pour la mesure
    /// en production). L'exclusion lecteur/écrivain qui protège le remap
    /// d'`ensure_room` est conservée par `.write()` sur `write_slot` ;
    /// seule la sérialisation lecteur/lecteur — qui n'a jamais protégé
    /// quoi que ce soit — disparaît.
    writers: DashMap<VecColSpec, Arc<RwLock<VecColumnWriter>>>,
    /// T2 get_node perf (plan b30650e2, gotcha 7940daa7): per-EntityKind
    /// snapshot of the registered keys, maintained at registration time.
    /// `iter_props_for_entity` runs on EVERY `get_node` — iterating and
    /// cloning the whole `by_key` DashMap there made bulk hydration pay
    /// O(all registered columns, both kinds) in iteration + key clones
    /// per node. Columns are registered rarely (first write / open), so
    /// a copy-on-write `Arc<Vec<_>>` snapshot is the right trade.
    keys_by_kind: DashMap<EntityKind, Arc<Vec<PropertyKey>>>,
}

impl VecColumnRegistry {
    pub(crate) fn new() -> Self {
        Self {
            by_key: DashMap::new(),
            writers: DashMap::new(),
            keys_by_kind: DashMap::new(),
        }
    }

    /// Record `key` in the per-kind key snapshot (idempotent). Must be
    /// called wherever `by_key` gains an entry.
    fn note_key(&self, key: &PropertyKey, ek: EntityKind) {
        let mut entry = self
            .keys_by_kind
            .entry(ek)
            .or_insert_with(|| Arc::new(Vec::new()));
        if !entry.iter().any(|k| k == key) {
            let mut v: Vec<PropertyKey> = (**entry).clone();
            v.push(key.clone());
            *entry = Arc::new(v);
        }
    }

    /// O(1) snapshot of the keys registered for `ek`.
    fn keys_for(&self, ek: EntityKind) -> Arc<Vec<PropertyKey>> {
        self.keys_by_kind
            .get(&ek)
            .map(|e| Arc::clone(&e))
            .unwrap_or_else(|| Arc::new(Vec::new()))
    }

    /// Rehydrate the registry from persisted specs. Takes the
    /// `vec_columns` list from [`crate::dict::DictSnapshot`] plus the
    /// property-key name table (indexed by `prop_key_id`). For each
    /// spec, opens a writer (creating the zone file if absent, but
    /// the file is expected to already exist from a prior session)
    /// and records the key→spec mapping.
    ///
    /// Missing files are tolerated: the writer will be created empty
    /// and the first read will return `None`. Mismatching on-disk
    /// headers are also tolerated — the writer inherits `n_slots=0`
    /// and overwrites on next write.
    pub(crate) fn hydrate_from_dict(
        &self,
        sub: &SubstrateFile,
        prop_key_names: &[String],
        specs: &[VecColSpec],
    ) -> SubstrateResult<()> {
        for spec in specs {
            let Some(name) = prop_key_names.get(spec.prop_key_id as usize) else {
                // Persisted spec references a prop_key_id past the
                // known table. Dict corruption or forward-compat
                // drift — skip with a log.
                tracing::warn!(
                    target: "substrate::vec_columns",
                    "hydrate: vec_column spec references unknown prop_key_id {} \
                     (only {} names known); skipping",
                    spec.prop_key_id,
                    prop_key_names.len()
                );
                continue;
            };
            let key = PropertyKey::new(name.as_str());
            let writer = VecColumnWriter::create(sub, *spec)?;
            self.writers.insert(*spec, Arc::new(RwLock::new(writer)));
            self.note_key(&key, spec.entity_kind);
            self.by_key.insert((key, spec.entity_kind), *spec);
        }
        Ok(())
    }

    /// Look up the spec for a (key, entity_kind) pair, if any. This
    /// is the cheap fast-path used by the read-side router — on a
    /// miss the caller falls through to the in-memory property map.
    pub(crate) fn spec_for(&self, key: &PropertyKey, ek: EntityKind) -> Option<VecColSpec> {
        self.by_key.get(&(key.clone(), ek)).map(|r| *r)
    }

    /// (2026-08-21) Résout l'écrivain d'une colonne UNE fois, pour un lot.
    ///
    /// Le chemin unitaire (`write` / `write_existing`) refait à CHAQUE vecteur :
    /// une allocation `PropertyKey`, une recherche dans `by_key` (verrou de
    /// shard + hachage), une recherche dans `writers` (idem), puis la prise du
    /// `RwLock` de la colonne. Pour une clé et une colonne identiques d'un
    /// appel à l'autre — le cas d'un enrichissement, 626 845 fois
    /// `_hilbert_features` — cinq sixièmes de ce travail sont répétés pour rien.
    ///
    /// Mesuré avant : 438 ns par écriture, dont ~10 ns de `memcpy`.
    ///
    /// Cette méthode rend l'`Arc` de l'écrivain ; l'appelant boucle ensuite sur
    /// `write_slot` sans plus rien résoudre. Le `RwLock` reste pris PAR
    /// ÉCRITURE et non pour tout le lot : c'est délibéré — le tenir sur la
    /// durée du lot bloquerait les lecteurs de la colonne pendant tout
    /// l'enrichissement, ce que ce chantier cherche précisément à éviter.
    ///
    /// Rend `None` si la colonne n'existe pas ou si la spec diverge ; l'appelant
    /// passe alors par `write`, qui crée ou produit l'erreur nommée.
    pub(crate) fn ecrivain_existant(
        &self,
        key: &PropertyKey,
        ek: EntityKind,
        prop_key_id: u16,
        dim: u32,
    ) -> Option<Arc<RwLock<VecColumnWriter>>> {
        let spec = VecColSpec {
            prop_key_id,
            entity_kind: ek,
            dim,
            dtype: VecDType::F32,
        };
        {
            let existant = self.by_key.get(&(key.clone(), ek))?;
            let ex = *existant;
            drop(existant);
            if ex != spec {
                return None;
            }
        }
        self.writers.get(&spec).map(|w| w.clone())
    }

    /// (2026-08-21) CHEMIN RAPIDE — ecrire dans une colonne DEJA enregistree,
    /// **sans le `SubstrateFile`** et donc sans le mutex global de la base.
    ///
    /// Pourquoi cette methode existe. `write()` ci-dessous prend `sub` en
    /// argument, ce qui oblige l'appelant (`SubstrateStore::route_vector_write`)
    /// a tenir `self.substrate.lock()` — le `Mutex<SubstrateFile>` GLOBAL — pour
    /// la duree de l'appel. Or `sub` ne sert qu'a **une** chose : construire le
    /// `VecColumnWriter` a la premiere ecriture de la colonne
    /// (`VecColumnWriter::create(sub, spec)`). `write_slot`, lui, n'a meme pas
    /// `sub` dans sa signature : il memcpy dans le mmap deja etabli.
    ///
    /// Consequence du contrat precedent : persister un vecteur par noeud sur une
    /// base de 626 845 noeuds prenait le verrou global 626 845 fois pour proteger
    /// une ressource utilisee une seule fois — une contention hachee sur tout le
    /// chemin chaud, en concurrence avec chaque lecteur de la base.
    ///
    /// Rend `None` quand la colonne n'existe pas encore, quand la spec ne
    /// correspond pas, ou quand le vecteur est invalide : dans ces trois cas
    /// l'appelant doit prendre le verrou et passer par `write()`, qui produit
    /// l'erreur exacte ou cree la colonne. Le `RwLock` par colonne, lui, est
    /// conserve : c'est LUI qui serialise legitimement deux ecritures sur la
    /// meme colonne.
    pub(crate) fn write_existing(
        &self,
        key: &PropertyKey,
        ek: EntityKind,
        prop_key_id: u16,
        slot: u32,
        vector: &[f32],
    ) -> Option<SubstrateResult<()>> {
        // Vecteur invalide : on laisse le chemin lent produire l'erreur nommee.
        if vector.is_empty() || vector.len() > u32::MAX as usize {
            return None;
        }
        let spec = VecColSpec {
            prop_key_id,
            entity_kind: ek,
            dim: vector.len() as u32,
            dtype: VecDType::F32,
        };
        // La spec doit etre DEJA enregistree ET identique. Une divergence part
        // au chemin lent, qui rend le message de rejet detaille.
        {
            let existing = self.by_key.get(&(key.clone(), ek))?;
            let ex = *existing;
            drop(existing);
            if ex != spec {
                return None;
            }
        }
        let writer = self.writers.get(&spec)?.clone();
        let bytes: &[u8] = bytemuck::cast_slice(vector);
        Some(writer.write().write_slot(slot, bytes))
    }

    /// Route a `Value::Vector` write for a node/edge slot. Creates
    /// the vec column on first use; rejects dim/dtype mismatches for
    /// an already-registered key. `prop_key_id` must already be
    /// interned in the store's `PropertyKeyRegistry`.
    pub(crate) fn write(
        &self,
        sub: &SubstrateFile,
        key: &PropertyKey,
        ek: EntityKind,
        prop_key_id: u16,
        slot: u32,
        vector: &[f32],
    ) -> SubstrateResult<()> {
        if vector.is_empty() {
            return Err(SubstrateError::WalBadFrame(
                "vec_column: cannot store empty vector".into(),
            ));
        }
        if vector.len() > u32::MAX as usize {
            return Err(SubstrateError::WalBadFrame(format!(
                "vec_column: vector length {} exceeds u32::MAX",
                vector.len()
            )));
        }
        let dim = vector.len() as u32;
        let spec = VecColSpec {
            prop_key_id,
            entity_kind: ek,
            dim,
            dtype: VecDType::F32,
        };

        // First: enforce spec-consistency per (key, ek). The first
        // vector determines the shape; later writes with a different
        // dim must be rejected rather than silently written to a
        // different file (which would leave the old column stranded).
        if let Some(existing) = self.by_key.get(&(key.clone(), ek)) {
            let ex = *existing;
            drop(existing);
            if ex != spec {
                return Err(SubstrateError::WalBadFrame(format!(
                    "vec_column: cannot change spec for key {:?} ({:?}): \
                     already registered as {:?}, now got {:?}",
                    key, ek, ex, spec
                )));
            }
        } else {
            self.note_key(key, ek);
            self.by_key.insert((key.clone(), ek), spec);
        }

        // Second: get-or-create the writer.
        let writer = if let Some(w) = self.writers.get(&spec) {
            w.clone()
        } else {
            let w = Arc::new(RwLock::new(VecColumnWriter::create(sub, spec)?));
            // `entry().or_insert_with` would race with another thread
            // creating the same spec. Use `entry` to be explicit.
            self.writers
                .entry(spec)
                .or_insert_with(|| w.clone())
                .clone()
        };

        // Third: write the payload. `write_slot` takes `&mut self`
        // so the write lock brackets the mmap mutation.
        let bytes: &[u8] = bytemuck::cast_slice(vector);
        writer.write().write_slot(slot, bytes)
    }

    /// Copy the vector for `slot` out of the column registered for
    /// `(key, ek)`. Returns `None` if the key is not a vec column,
    /// if the slot is past the writer's high-water mark, or if the
    /// dtype is not F32. Other dtypes are not surfaced to
    /// `Value::Vector` today (it is `Arc<[f32]>`).
    pub(crate) fn read(&self, key: &PropertyKey, ek: EntityKind, slot: u32) -> Option<Arc<[f32]>> {
        let spec = self.spec_for(key, ek)?;
        let writer = self.writers.get(&spec)?.clone();
        let guard = writer.read();
        let data: &[f32] = guard.read_slot_f32(slot)?;
        // Copy out into an owned Arc<[f32]>. The borrow dies with
        // the lock guard, so we cannot hand out the mmap slice.
        Some(Arc::from(data))
    }

    /// Like [`Self::read`] but applies the all-zero-sentinel presence
    /// check on the mmap borrow, INSIDE the column lock, BEFORE the
    /// heap copy (T2 get_node perf). Absent slots (grown past, never
    /// written) cost a linear scan of `dim` floats but zero allocation
    /// — on bulk hydration most registered columns are absent for any
    /// given node, so this is the common case.
    pub(crate) fn read_nonzero(
        &self,
        key: &PropertyKey,
        ek: EntityKind,
        slot: u32,
    ) -> Option<Arc<[f32]>> {
        let spec = self.spec_for(key, ek)?;
        let writer = self.writers.get(&spec)?.clone();
        let guard = writer.read();
        let data: &[f32] = guard.read_slot_f32(slot)?;
        if data.iter().all(|x| *x == 0.0) {
            return None;
        }
        Some(Arc::from(data))
    }

    /// T17k — bulk iteration of every registered vec column whose
    /// `EntityKind` matches `ek`, yielding the `(PropertyKey, vector)`
    /// pair when `slot` has a meaningful value on that column.
    ///
    /// **Presence semantics.** Vec columns are dense: writing slot `N`
    /// grows `n_slots` past any intermediate slots, whose bytes are
    /// zero-initialised by the mmap. To distinguish "node really has
    /// an all-zero embedding" from "slot was grown past, never
    /// written", this iterator uses the all-zero sentinel convention:
    /// vectors whose every element equals `0.0` are treated as absent.
    /// In practice legitimate embeddings are never exactly all-zero
    /// (cosine normalisation would be undefined), so this convention
    /// matches production use. Callers who need to distinguish the
    /// two must call [`Self::read`] with the specific key and inspect.
    ///
    /// Returns owned pairs because the underlying mmap borrows die
    /// with the per-column lock guard — hot paths that need the
    /// whole property map of a node call this once and consume.
    ///
    /// Cost: `O(num_matching_cols × (lock + dim floats compared))`.
    /// On PO Node typically ≤ 10 vec columns → ~10 μs.
    pub(crate) fn iter_props_for_entity(
        &self,
        ek: EntityKind,
        slot: u32,
    ) -> Vec<(PropertyKey, Arc<[f32]>)> {
        // T2 get_node perf: O(1) Arc-cloned key snapshot instead of a
        // full DashMap iteration + key clones per call, and the
        // all-zero sentinel is checked on the mmap borrow INSIDE the
        // column lock (`read_nonzero`) so absent columns cost a scan,
        // not a scan + a heap copy of up to `dim` floats.
        let keys = self.keys_for(ek);
        keys.iter()
            .filter_map(|k| Some((k.clone(), self.read_nonzero(k, ek, slot)?)))
            .collect()
    }

    /// Iterate every currently-registered spec. Used by
    /// `SubstrateStore::build_dict_snapshot` to populate the v3
    /// `vec_columns` list.
    pub(crate) fn specs_snapshot(&self) -> Vec<VecColSpec> {
        let mut out: Vec<VecColSpec> = self.writers.iter().map(|e| *e.key()).collect();
        // Deterministic order for snapshot roundtrips + easier diffing.
        out.sort_by_key(|s| (s.entity_kind as u8, s.prop_key_id, s.dtype as u8, s.dim));
        out
    }

    /// Durability barrier: walk every open writer and `sync` it
    /// (header + msync + fsync). Called from `SubstrateStore::flush`
    /// so the vec columns become durable at the same cadence as the
    /// rest of the substrate.
    pub(crate) fn sync_all(&self) -> SubstrateResult<()> {
        for entry in self.writers.iter() {
            entry.value().write().sync()?;
        }
        Ok(())
    }
}

/// Chantier SlotRemappable — chemin VIF : les writers ouverts du registre
/// portent l'état authoritaire (n_slots non synchronisés compris). Chaque
/// colonne nœud est permutée out-of-place puis rendue durable en-tête en
/// dernier (CRC + epoch) par [`VecColumnWriter::remap_node_slots`].
/// Le chemin REJEU ne passe jamais ici (registre inexistant après crash) —
/// il passe par `crate::vec_column::remap_node_file_for_replay`.
impl crate::slot_remap::SlotRemappable for VecColumnRegistry {
    fn remap_name(&self) -> &'static str {
        "vec_columns"
    }

    fn remap_node_slots(
        &self,
        substrate: &Arc<parking_lot::Mutex<SubstrateFile>>,
        old_to_new: &[u32],
        epoch: u32,
        _ctx: crate::slot_remap::RemapContext,
    ) -> SubstrateResult<()> {
        // Verrou pris PAR COLONNE, jamais tenu par l'appelant : c'est ce
        // qui evite l'interblocage avec les implementeurs qui reprennent
        // le mutex en interne (cf. la doc du trait `SlotRemappable`).
        for entry in self.writers.iter() {
            if entry.key().entity_kind != EntityKind::Node {
                continue;
            }
            let sub = substrate.lock();
            entry
                .value()
                .write()
                .remap_node_slots(&sub, old_to_new, epoch)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::SubstrateFile;

    fn pk(s: &str) -> PropertyKey {
        PropertyKey::new(s)
    }

    #[test]
    fn write_then_read_node_roundtrip() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();

        let v = vec![1.0_f32, 2.0, 3.0, 4.0];
        reg.write(&sub, &pk("embedding"), EntityKind::Node, 0, 5, &v)
            .unwrap();

        let got = reg.read(&pk("embedding"), EntityKind::Node, 5).unwrap();
        assert_eq!(got.as_ref(), &v[..]);
    }

    #[test]
    fn iter_props_for_entity_round_trip() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();
        // Three vec columns on Node, one on Edge (must not leak).
        reg.write(
            &sub,
            &pk("embedding"),
            EntityKind::Node,
            0,
            7,
            &[1.0_f32; 4],
        )
        .unwrap();
        reg.write(&sub, &pk("hilbert"), EntityKind::Node, 1, 7, &[2.0_f32; 4])
            .unwrap();
        reg.write(&sub, &pk("kernel"), EntityKind::Node, 2, 7, &[3.0_f32; 4])
            .unwrap();
        reg.write(&sub, &pk("edge_vec"), EntityKind::Edge, 3, 7, &[9.0_f32; 4])
            .unwrap();

        let props = reg.iter_props_for_entity(EntityKind::Node, 7);
        assert_eq!(props.len(), 3, "expected 3 Node vec props, got {props:?}");
        let keys: std::collections::HashSet<_> =
            props.iter().map(|(k, _)| k.as_str().to_string()).collect();
        assert!(keys.contains("embedding"));
        assert!(keys.contains("hilbert"));
        assert!(keys.contains("kernel"));
        assert!(!keys.contains("edge_vec"), "Edge vec leaked into Node iter");
    }

    #[test]
    fn iter_props_for_entity_skips_absent_slots() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();
        reg.write(
            &sub,
            &pk("embedding"),
            EntityKind::Node,
            0,
            5,
            &[1.0_f32; 3],
        )
        .unwrap();
        reg.write(&sub, &pk("hilbert"), EntityKind::Node, 1, 9, &[2.0_f32; 3])
            .unwrap();
        // Slot 5 has "embedding", slot 9 has "hilbert", slot 7 has NEITHER.
        let props_7 = reg.iter_props_for_entity(EntityKind::Node, 7);
        assert!(props_7.is_empty(), "slot 7 has no props, got {props_7:?}");
        let props_5 = reg.iter_props_for_entity(EntityKind::Node, 5);
        assert_eq!(props_5.len(), 1);
        assert_eq!(props_5[0].0.as_str(), "embedding");
    }

    #[test]
    fn iter_props_empty_registry_returns_empty() {
        let reg = VecColumnRegistry::new();
        assert!(reg.iter_props_for_entity(EntityKind::Node, 42).is_empty());
    }

    #[test]
    fn read_missing_key_returns_none() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();
        let v = vec![0.5_f32; 3];
        reg.write(&sub, &pk("embedding"), EntityKind::Node, 0, 0, &v)
            .unwrap();
        assert!(reg.read(&pk("unknown"), EntityKind::Node, 0).is_none());
    }

    #[test]
    fn read_missing_slot_returns_none() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();
        let v = vec![0.5_f32; 3];
        reg.write(&sub, &pk("embedding"), EntityKind::Node, 0, 0, &v)
            .unwrap();
        assert!(reg.read(&pk("embedding"), EntityKind::Node, 99).is_none());
    }

    #[test]
    fn node_and_edge_keys_are_independent() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();

        let v_node = vec![1.0_f32, 2.0, 3.0];
        let v_edge = vec![10.0_f32, 20.0, 30.0];
        reg.write(&sub, &pk("weight"), EntityKind::Node, 0, 0, &v_node)
            .unwrap();
        reg.write(&sub, &pk("weight"), EntityKind::Edge, 0, 0, &v_edge)
            .unwrap();

        let got_node = reg.read(&pk("weight"), EntityKind::Node, 0).unwrap();
        let got_edge = reg.read(&pk("weight"), EntityKind::Edge, 0).unwrap();
        assert_eq!(got_node.as_ref(), &v_node[..]);
        assert_eq!(got_edge.as_ref(), &v_edge[..]);
    }

    #[test]
    fn dim_mismatch_is_rejected() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();
        let v4 = vec![0.5_f32; 4];
        let v3 = vec![0.1_f32; 3];
        reg.write(&sub, &pk("k"), EntityKind::Node, 0, 0, &v4)
            .unwrap();
        let err = reg
            .write(&sub, &pk("k"), EntityKind::Node, 0, 1, &v3)
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("cannot change spec"), "unexpected err: {msg}");
    }

    #[test]
    fn empty_vector_is_rejected() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();
        let err = reg
            .write(&sub, &pk("k"), EntityKind::Node, 0, 0, &[])
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("empty"), "unexpected err: {msg}");
    }

    #[test]
    fn specs_snapshot_is_deterministic() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();
        let v = vec![0.5_f32; 4];
        // Insert in a scrambled order.
        reg.write(&sub, &pk("b"), EntityKind::Edge, 2, 0, &v)
            .unwrap();
        reg.write(&sub, &pk("a"), EntityKind::Node, 0, 0, &v)
            .unwrap();
        reg.write(&sub, &pk("c"), EntityKind::Node, 1, 0, &v)
            .unwrap();

        let specs = reg.specs_snapshot();
        assert_eq!(specs.len(), 3);
        // Sorted by (entity_kind, prop_key_id, dtype, dim):
        // entity_kind Node(0) first, then Edge(1).
        assert_eq!(specs[0].prop_key_id, 0);
        assert_eq!(specs[0].entity_kind, EntityKind::Node);
        assert_eq!(specs[1].prop_key_id, 1);
        assert_eq!(specs[1].entity_kind, EntityKind::Node);
        assert_eq!(specs[2].prop_key_id, 2);
        assert_eq!(specs[2].entity_kind, EntityKind::Edge);
    }

    #[test]
    fn sync_all_makes_data_durable_to_a_fresh_reader() {
        use crate::vec_column::VecColumnReader;
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = VecColumnRegistry::new();
        let v = vec![7.0_f32; 5];
        reg.write(&sub, &pk("emb"), EntityKind::Node, 0, 3, &v)
            .unwrap();
        reg.sync_all().unwrap();

        let spec = reg
            .spec_for(&pk("emb"), EntityKind::Node)
            .expect("spec should be registered");
        let r = VecColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.n_slots(), 4); // slots 0..=3 allocated, so n_slots = 4
        assert_eq!(r.read_slot_f32(3).unwrap(), &v[..]);
    }

    #[test]
    fn hydrate_from_dict_recovers_registered_key() {
        // Write + sync a column, then simulate a reopen via a fresh
        // registry that re-hydrates from the (spec, prop-key-name)
        // list that would have been persisted in the dict.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg1 = VecColumnRegistry::new();
        let v = vec![1.0_f32, 2.0, 3.0];
        reg1.write(&sub, &pk("embedding"), EntityKind::Node, 0, 7, &v)
            .unwrap();
        reg1.sync_all().unwrap();
        let specs = reg1.specs_snapshot();
        drop(reg1);

        // Fresh registry. Hydrate from what the dict would carry.
        let reg2 = VecColumnRegistry::new();
        reg2.hydrate_from_dict(&sub, &["embedding".to_string()], &specs)
            .unwrap();

        let got = reg2.read(&pk("embedding"), EntityKind::Node, 7).unwrap();
        assert_eq!(got.as_ref(), &v[..]);
    }
}
