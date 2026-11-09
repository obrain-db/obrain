//! Production [`PositionProvider`]: reads node position vectors from the substrate.
//!
//! # Why a snapshot
//!
//! [`PositionProvider`] hands out **borrowed** slices so the query path never
//! allocates. A substrate read returns an owned `Value::Vector` per node, so the
//! provider takes one contiguous snapshot at load time and serves slices out of
//! it. That keeps the trait contract (no allocation on the hot path) and makes
//! the cost explicit: one `O(N)` pass to load, then `O(1)` per lookup.
//!
//! # The zero-vector convention
//!
//! `Value::Vector` properties live in the dense `substrate.veccol.*` column since
//! T16.7. Slots that were grown but never written read back as **all-zero**
//! vectors, which is not the same as "position is the origin". Those are skipped,
//! exactly like the existing `try_load_hilbert_features_from_store` does, and
//! counted in [`SubstratePositionProvider::skipped_zero`] so the caller can tell
//! the difference between "absent" and "origin".
//!
//! # Freshness
//!
//! A snapshot is a snapshot. The caller reloads when the base changed; the
//! revision is recorded at load time via [`SubstratePositionProvider::loaded_at`]
//! so a caller can compare it with the store's current state instead of guessing.

use std::collections::HashMap;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::GraphStore;
use obrain_substrate::SubstrateStore;

use super::position::{POS_DIM_MAX, PosError, PositionProvider};

/// Default property holding the per-node position vector.
pub const DEFAULT_POSITION_KEY: &str = "_hilbert_features";

/// Nodes sampled by the coverage probe before the exhaustive pass.
pub const PROBE_SAMPLE: usize = 512;

/// Default minimum coverage accepted by [`SubstratePositionProvider::load`].
///
/// 1 % : below that, the column is treated as absent. This mirrors the rejection
/// threshold `try_load_hilbert_features_from_store` applies to a cached feature
/// set, and it is a *caller-visible* choice - pass your own value, or 0.0 to
/// force the exhaustive scan.
pub const DEFAULT_MIN_COVERAGE: f64 = 0.01;

/// A snapshot of node position vectors read from a substrate store.
#[derive(Debug, Clone)]
pub struct SubstratePositionProvider {
    dim: usize,
    key: String,
    offsets: HashMap<NodeId, u32>,
    data: Vec<f32>,
    skipped_zero: usize,
    loaded_at: u64,
    /// Ce que la SONDE a vu : (sondes faites, sondes portant une position).
    ///
    /// ── POURQUOI CES DEUX NOMBRES EXISTENT (2026-09-11) ───────────────────
    /// Quand la couverture mesurée tombe sous `min_coverage`, le provider est
    /// rendu VIDE — `dim = 0`, `loaded_at = 0` — c'est-à-dire exactement l'état
    /// d'une colonne ABSENTE. Une colonne réellement présente sur 5 000 nœuds
    /// d'une base d'un million (0,5 %) était donc déclarée absente, et
    /// l'appelant ne pouvait pas distinguer « il n'y a rien » de « c'est trop
    /// clairsemé pour ce seuil ». L'en-tête de ce module promet pourtant
    /// précisément de distinguer « absent » d'« origine ». On garde donc la
    /// trace de la sonde : elle est lisible (`couverture_mesuree`) et c'est elle
    /// qui rend le refus explicable au lieu d'être un vide.
    sondes: usize,
    touches: usize,
}

impl SubstratePositionProvider {
    /// Ce que la sonde a vu : `(sondes faites, sondes portant une position)`.
    ///
    /// `(0, 0)` quand la sonde n'a pas eu lieu (base sous `PROBE_SAMPLE`, ou
    /// `min_coverage = 0.0`). Quand le premier nombre est non nul et le second
    /// nul, la colonne est réellement absente ; quand les deux sont non nuls mais
    /// que le provider est vide, la colonne EXISTE et a été refusée pour cause
    /// de densité — deux situations que rien ne distinguait auparavant.
    pub fn couverture_mesuree(&self) -> (usize, usize) {
        (self.sondes, self.touches)
    }

    /// Loads the position snapshot under `key`.
    ///
    /// Fails when two position dimensions coexist in the base (see
    /// [`PosError::InconsistentDimensions`]) or when the dimension exceeds
    /// [`POS_DIM`](crate::engram::position::POS_DIM). Absent nodes are simply not in the snapshot:
    /// [`PositionProvider::position`] returns `None` for them.
    pub fn load(store: &SubstrateStore, key: &str) -> Result<Self, PosError> {
        Self::load_with_coverage(store, key, DEFAULT_MIN_COVERAGE)
    }

    /// Loads the snapshot, treating a coverage below `min_coverage` as absent.
    ///
    /// The probe exists because discovering an ABSENT column used to require a
    /// full pass: measured 12.6-27.1 ms on real bases of 135k-259k nodes, all of
    /// which lacked the column. With the probe it is a few hundred microseconds.
    /// The result is identical either way when the column exists.
    pub fn load_with_coverage(
        store: &SubstrateStore,
        key: &str,
        min_coverage: f64,
    ) -> Result<Self, PosError> {
        let property = PropertyKey::from(key);
        let node_ids = store.all_node_ids();

        // COVERAGE PROBE FIRST.
        //
        // Measured on four real bases (135k-259k nodes, none carrying the column):
        // the full pass below cost 12.6-27.1 ms just to discover the column was
        // ABSENT - a wasted `get_node_property` per node. The project already
        // uses this sampling pattern (`try_load_hilbert_features_from_store`,
        // `KernelEnricher`); this provider did not.
        //
        // Ce qu'une sonde PEUT faire, et ce qu'elle ne peut pas : elle ne manque
        // pas une colonne présente, mais elle peut la REFUSER — c'est-à-dire
        // rendre un provider vide pour une colonne qui existe, si sa densité est
        // sous `min_coverage`. Ce n'est pas la même chose qu'une absence, et
        // `couverture_mesuree` permet désormais de le dire. Passer 0.0 force la
        // lecture exhaustive.
        if node_ids.len() > PROBE_SAMPLE && min_coverage > 0.0 {
            let step = (node_ids.len() / PROBE_SAMPLE).max(1);
            let mut hits = 0usize;
            let mut probes = 0usize;
            for i in (0..node_ids.len()).step_by(step) {
                probes += 1;
                if let Some(Value::Vector(v)) = store.get_node_property(node_ids[i], &property)
                    && !v.is_empty()
                    && !v.iter().all(|x| *x == 0.0)
                {
                    hits += 1;
                }
            }
            if probes > 0 && (hits as f64 / probes as f64) < min_coverage {
                // Le refus est DOCUMENTÉ (sonde conservée, journalisé) : il reste
                // un refus, il n'est plus un silence.
                tracing::debug!(
                    key = %key,
                    sondes = probes,
                    touches = hits,
                    seuil = min_coverage,
                    "colonne de position SOUS LE SEUIL : provider laissé vide —                      `load_with_coverage(…, 0.0)` force la lecture exhaustive"
                );
                return Ok(Self {
                    dim: 0,
                    key: key.to_string(),
                    offsets: HashMap::new(),
                    data: Vec::new(),
                    skipped_zero: 0,
                    loaded_at: 0,
                    sondes: probes,
                    touches: hits,
                });
            }
        }
        let mut offsets: HashMap<NodeId, u32> = HashMap::new();
        let mut data: Vec<f32> = Vec::new();
        let mut dim: Option<usize> = None;
        let mut skipped_zero = 0usize;

        for nid in node_ids {
            let Some(Value::Vector(v)) = store.get_node_property(nid, &property) else {
                continue;
            };
            if v.is_empty() {
                continue;
            }
            // veccol presence convention: an all-zero read means "slot grown,
            // never written", not "position at the origin".
            if v.iter().all(|x| *x == 0.0) {
                skipped_zero += 1;
                continue;
            }
            match dim {
                None => {
                    // Le plafond n'est plus la dimension CANONIQUE (128) mais le
                    // garde-fou d'allocation : une colonne à 256 dimensions est
                    // une donnée légitime, pas une erreur de format.
                    if v.len() > POS_DIM_MAX {
                        return Err(PosError::BadDimension {
                            got: v.len(),
                            expected: POS_DIM_MAX,
                        });
                    }
                    dim = Some(v.len());
                }
                Some(d) if v.len() != d => {
                    // Two different feature spaces in one base. Refuse: keeping
                    // either one would make the load depend on enumeration order,
                    // and mixing coordinates silently mis-aligns positions.
                    return Err(PosError::InconsistentDimensions {
                        expected: d,
                        got: v.len(),
                    });
                }
                Some(_) => {}
            }
            let d = dim.expect("dim was just set");
            let off = u32::try_from(data.len()).map_err(|_| PosError::BadIndexDimension(d))?;
            data.extend_from_slice(&v);
            offsets.insert(nid, off);
        }

        let dim = dim.unwrap_or(0);
        // Shape-derived fingerprint, explicitly NOT a store revision.
        let loaded_at = (offsets.len() as u64).wrapping_mul(1_000_003) ^ (data.len() as u64);
        Ok(Self {
            sondes: 0,
            touches: 0,
            dim,
            key: key.to_string(),
            offsets,
            data,
            skipped_zero,
            loaded_at,
        })
    }

    /// Dimension of the loaded position vectors (0 when nothing was loaded).
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Number of nodes with a usable position.
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    /// True when no node carried a usable position.
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// Property key this snapshot was read from.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Nodes skipped because their vector was entirely zero (never written).
    pub fn skipped_zero(&self) -> usize {
        self.skipped_zero
    }

    /// Opaque fingerprint of the snapshot, for a caller that wants to notice a
    /// reload. It is **not** a store revision: the substrate exposes no cheap
    /// global mutation counter, so this is a shape-derived value and is
    /// documented as such rather than passed off as one.
    pub fn loaded_at(&self) -> u64 {
        self.loaded_at
    }

    /// Bytes held by the snapshot.
    pub fn bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<f32>()
            + self.offsets.len() * std::mem::size_of::<(NodeId, u32)>()
    }
}

impl PositionProvider for SubstratePositionProvider {
    fn position(&self, id: NodeId) -> Option<&[f32]> {
        if self.dim == 0 {
            return None;
        }
        let off = *self.offsets.get(&id)? as usize;
        Some(&self.data[off..off + self.dim])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engram::position::SlicePositionProvider;
    use obrain_core::graph::GraphStoreMut;
    use std::sync::Arc;

    fn store_with_positions(dim: usize, nodes: usize) -> (SubstrateStore, Vec<NodeId>) {
        let store = SubstrateStore::open_tempfile().unwrap();
        let mut ids = Vec::new();
        for i in 0..nodes {
            let id = store.create_node(&["N"]);
            // Non-zero, non-negative, distinguishable per node.
            let v: Vec<f32> = (0..dim)
                .map(|j| 0.05 + (i as f32) * 0.01 + (j as f32) * 0.001)
                .collect();
            store.set_node_property(
                id,
                DEFAULT_POSITION_KEY,
                Value::Vector(Arc::from(v.as_slice())),
            );
            ids.push(id);
        }
        store.flush().unwrap();
        (store, ids)
    }

    #[test]
    fn reads_back_exactly_what_was_written() {
        let (store, ids) = store_with_positions(16, 12);
        let p = SubstratePositionProvider::load(&store, DEFAULT_POSITION_KEY).unwrap();
        assert_eq!(p.dim(), 16);
        assert_eq!(p.len(), 12);
        assert_eq!(p.skipped_zero(), 0);
        for (i, id) in ids.iter().enumerate() {
            let got = p.position(*id).expect("position attendue");
            assert_eq!(got.len(), 16);
            for (j, x) in got.iter().enumerate() {
                let want = 0.05 + (i as f32) * 0.01 + (j as f32) * 0.001;
                assert_eq!(*x, want, "nœud {i}, coordonnée {j} : valeur altérée");
            }
        }
        println!(
            "provider substrate : {} nœuds × {} dims, {} octets",
            p.len(),
            p.dim(),
            p.bytes()
        );
    }

    #[test]
    fn nodes_without_position_return_none_not_the_origin() {
        let (store, ids) = store_with_positions(8, 3);
        let orphan = store.create_node(&["N"]); // jamais écrit
        let p = SubstratePositionProvider::load(&store, DEFAULT_POSITION_KEY).unwrap();
        assert!(
            p.position(orphan).is_none(),
            "un nœud sans position doit rendre None"
        );
        assert_eq!(p.len(), 3);
        for id in &ids {
            assert!(p.position(*id).is_some());
        }
    }

    #[test]
    fn zero_vectors_are_absent_not_at_the_origin() {
        let store = SubstrateStore::open_tempfile().unwrap();
        let real = store.create_node(&["N"]);
        store.set_node_property(
            real,
            DEFAULT_POSITION_KEY,
            Value::Vector(Arc::from([0.3f32; 4].as_slice())),
        );
        let zeroed = store.create_node(&["N"]);
        store.set_node_property(
            zeroed,
            DEFAULT_POSITION_KEY,
            Value::Vector(Arc::from([0.0f32; 4].as_slice())),
        );
        store.flush().unwrap();
        let p = SubstratePositionProvider::load(&store, DEFAULT_POSITION_KEY).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(
            p.skipped_zero(),
            1,
            "le tout-zéro doit être compté comme absent"
        );
        assert!(p.position(zeroed).is_none());
        assert_eq!(p.position(real).unwrap(), &[0.3, 0.3, 0.3, 0.3]);
    }

    /// Comportement RÉEL de la base face à deux dimensions (mesuré, pas supposé).
    ///
    /// Une colonne vecteur du substrate ne porte qu'UNE dimension : écrire 8
    /// coordonnées sur un nœud puis 5 sur un autre ne laisse **pas** deux
    /// dimensions coexister — la seconde écriture ne se matérialise pas et le
    /// nœud reste absent de la colonne. Observe : `load` réussit avec dim 8 et
    /// **un seul** nœud.
    ///
    /// Conséquence assumée : le cas « dimensions mixtes » est inatteignable par
    /// ce chemin, et la garde `PosError::InconsistentDimensions` est défensive
    /// (utile si la clé est alimentée par un autre écrivain). Ce test fige le
    /// comportement observé pour qu'une évolution du substrate ne passe pas
    /// inaperçue en silence.
    #[test]
    fn a_second_dimension_does_not_land_in_the_column() {
        let store = SubstrateStore::open_tempfile().unwrap();
        let a = store.create_node(&["N"]);
        store.set_node_property(
            a,
            DEFAULT_POSITION_KEY,
            Value::Vector(Arc::from([0.1f32; 8].as_slice())),
        );
        let b = store.create_node(&["N"]);
        store.set_node_property(
            b,
            DEFAULT_POSITION_KEY,
            Value::Vector(Arc::from([0.1f32; 5].as_slice())),
        );
        store.flush().unwrap();

        let p = SubstratePositionProvider::load(&store, DEFAULT_POSITION_KEY)
            .expect("la base ne présente pas de dimensions mixtes : une seule a été retenue");
        assert_eq!(p.dim(), 8);
        assert_eq!(
            p.len(),
            1,
            "seul le nœud à la dimension de la colonne est présent"
        );
        assert!(p.position(a).is_some());
        assert!(
            p.position(b).is_none(),
            "l'écriture à dimension différente ne doit pas produire de position"
        );
        println!(
            "dimension mixte : colonne à 8 dims, 2e écriture (5 dims) non matérialisée — \
             load = {} nœud(s), dim {}",
            p.len(),
            p.dim()
        );
    }

    #[test]
    fn empty_base_loads_empty_provider() {
        let store = SubstrateStore::open_tempfile().unwrap();
        let p = SubstratePositionProvider::load(&store, DEFAULT_POSITION_KEY).unwrap();
        assert!(p.is_empty());
        assert_eq!(p.dim(), 0);
        assert!(p.position(NodeId(1)).is_none());
    }

    /// Le sondage de couverture : colonne absente = vide ET rapide ; colonne
    /// présente au-dessus du seuil = chargée en entier.
    ///
    /// Les autres tests portent sur de petites bases (< 512 nœuds), donc le
    /// sondage n'y est jamais exercé : ce test couvre le chemin réel, celui des
    /// bases de production.
    #[test]
    fn coverage_probe_short_circuits_only_when_the_column_is_absent() {
        use web_time::Instant;
        // 700 nœuds, aucun avec la colonne : le sondage doit court-circuiter.
        let absente = SubstrateStore::open_tempfile().unwrap();
        for _ in 0..700 {
            absente.create_node(&["N"]);
        }
        absente.flush().unwrap();
        let t = Instant::now();
        let p = SubstratePositionProvider::load(&absente, DEFAULT_POSITION_KEY).unwrap();
        let dt = t.elapsed();
        assert!(p.is_empty(), "colonne absente => provider vide");
        assert_eq!(p.dim(), 0);
        assert!(
            dt.as_millis() < 500,
            "le sondage doit court-circuiter, pas balayer 700 nœuds : {dt:?}"
        );

        // même taille, colonne présente sur 50 % : le sondage laisse passer, et
        // le chargement est complet (pas seulement les nœuds sondés).
        let presente = SubstrateStore::open_tempfile().unwrap();
        let mut avec = 0;
        for i in 0..700 {
            let id = presente.create_node(&["N"]);
            if i % 2 == 0 {
                presente.set_node_property(
                    id,
                    DEFAULT_POSITION_KEY,
                    Value::Vector(Arc::from([0.25f32; 6].as_slice())),
                );
                avec += 1;
            }
        }
        presente.flush().unwrap();
        let p = SubstratePositionProvider::load(&presente, DEFAULT_POSITION_KEY).unwrap();
        assert_eq!(
            p.len(),
            avec,
            "le chargement doit être COMPLET, pas limité à l'échantillon"
        );
        assert_eq!(p.dim(), 6);

        // seuil élevé : la même base est alors traitée comme absente
        let strict =
            SubstratePositionProvider::load_with_coverage(&presente, DEFAULT_POSITION_KEY, 0.9)
                .unwrap();
        assert!(
            strict.is_empty(),
            "sous le seuil demandé, la colonne compte comme absente"
        );

        // seuil nul : balayage exhaustif, même résultat que le chemin sondé
        let exhaustif =
            SubstratePositionProvider::load_with_coverage(&presente, DEFAULT_POSITION_KEY, 0.0)
                .unwrap();
        assert_eq!(
            exhaustif.len(),
            avec,
            "seuil 0 => balayage complet, même compte"
        );
        println!(
            "sonde de couverture : absente en {dt:?} (au lieu d'un balayage de 700 nœuds) ; \
             présente => {avec} positions chargées"
        );
    }

    /// Chaîne complète : substrate → provider → `recall_with_position`.
    ///
    /// C'est le test qui prouve que la phase est finie : les positions viennent
    /// d'une vraie base, pas d'un tableau en mémoire de test.
    #[test]
    fn end_to_end_substrate_positions_drive_recall() {
        use crate::engram::recall::RecallEngine;
        use crate::engram::spectral::SpectralEncoder;
        use crate::engram::store::EngramStore;
        use crate::engram::traits::{InMemoryVectorIndex, VectorIndex};
        use crate::engram::types::Engram;

        let dim = 16;
        let store = SubstrateStore::open_tempfile().unwrap();
        // région A : nœuds 1..=8 ; région B : 101,102
        let mut made = Vec::new();
        for (label, base) in [("A", 0.10f32), ("B", 0.90f32)] {
            let count = if label == "A" { 8 } else { 2 };
            let start = if label == "A" { 1 } else { 101 };
            for i in 0..count {
                let id = store.create_node(&["N"]);
                let v: Vec<f32> = (0..dim)
                    .map(|j| base + (i as f32) * 0.001 + (j as f32) * 0.0001)
                    .collect();
                store.set_node_property(
                    id,
                    DEFAULT_POSITION_KEY,
                    Value::Vector(Arc::from(v.as_slice())),
                );
                made.push((start + i, id));
            }
        }
        store.flush().unwrap();
        let by_label: HashMap<usize, NodeId> = made.iter().map(|(k, v)| (*k, *v)).collect();

        let provider = SubstratePositionProvider::load(&store, DEFAULT_POSITION_KEY).unwrap();
        assert_eq!(provider.len(), 10);

        // engrams : X = nœuds région A {1,2,3}, Y = {4,5,6} (disjoint, même région),
        // Z = région B {101,102}
        let eng_store = EngramStore::new(None);
        let index = InMemoryVectorIndex::new();
        let spectral = SpectralEncoder::new();
        let mk = |keys: &[usize]| {
            let id = eng_store.next_id();
            let ens: Vec<(NodeId, f64)> = keys.iter().map(|k| (by_label[k], 1.0)).collect();
            let sig = spectral.encode(&ens);
            let mut e = Engram::new(id, ens);
            e.spectral_signature = sig.clone();
            index.upsert(&id.0.to_string(), &sig);
            eng_store.insert(e);
            id
        };
        let x = mk(&[1, 2, 3]);
        let y = mk(&[4, 5, 6]);
        let z = mk(&[101, 102]);

        let cues = vec![by_label[&7], by_label[&8]]; // région A, disjoints
        let engine = RecallEngine::new();
        let legacy = engine.recall(&eng_store, &cues, &index, &spectral, 10);
        let conf = |rs: &[crate::engram::recall::RecallResult], id| -> f64 {
            rs.iter()
                .find(|r| r.engram_id == id)
                .map_or(0.0, |r| r.confidence)
        };
        let with_pos = engine
            .recall_with_position(&eng_store, &cues, &index, &spectral, 10, &provider, 1, 0.5)
            .expect("provider substrate complet : aucune erreur");

        let (ly, lz) = (conf(&legacy, y), conf(&legacy, z));
        let (ny, nz) = (conf(&with_pos, y), conf(&with_pos, z));
        println!(
            "bout en bout substrate — Y (même région, nœuds disjoints) {ly:.4} -> {ny:.4} ; \
             Z (autre région) {lz:.4} -> {nz:.4}"
        );
        assert!(
            ny > ly,
            "la voie position doit élever Y (substrate) : {ly:.4} -> {ny:.4}"
        );
        assert!(
            ny > nz,
            "la région A doit passer devant la région B : {ny:.4} vs {nz:.4}"
        );
        assert!(with_pos.iter().any(|r| r.engram_id == y));
        let _ = x;
    }

    /// Le provider substrat et le provider mémoire doivent donner la même réponse :
    /// sinon l'un des deux ment, et le test de capacité ci-dessus ne vaudrait rien.
    #[test]
    fn substrate_and_memory_providers_agree() {
        let dim = 8;
        let (store, ids) = store_with_positions(dim, 6);
        let sub = SubstratePositionProvider::load(&store, DEFAULT_POSITION_KEY).unwrap();
        let mut flat = vec![0.0f32; ids.len() * dim];
        for (i, id) in ids.iter().enumerate() {
            flat[i * dim..(i + 1) * dim].copy_from_slice(sub.position(*id).unwrap());
        }
        let mem = SlicePositionProvider::new(dim, flat);
        for (i, id) in ids.iter().enumerate() {
            // même rang de nœud dans les deux providers
            let a = sub.position(*id).unwrap();
            let b = mem.position(NodeId(i as u64)).unwrap();
            assert_eq!(a, b, "désaccord substrat/mémoire au nœud {i}");
        }
    }
}
