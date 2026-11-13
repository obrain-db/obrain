//! Write-through path: WAL first, then mmap mutation.
//!
//! This is the minimal primitive used by `SubstrateStore` (T4) to guarantee
//! that every on-disk mutation is preceded by a durable WAL record. The
//! invariant is:
//!
//! ```text
//!   durable WAL record committed
//!             ↓
//!     mmap mutation visible
//!             ↓
//!   msync dirty range (lazy, batched)
//! ```
//!
//! A crash between (1) and (2) is recovered by replay (see
//! [`crate::replay`]). A crash between (2) and (3) is harmless — the mmap
//! edits are reapplied at replay from the WAL.
//!
//! ## Scope
//!
//! This module intentionally exposes only a small surface:
//!
//! * [`Writer::write_node`] / [`Writer::update_node`] / [`Writer::tombstone_node`]
//!   / [`Writer::read_node`] — node slot primitives.
//! * [`Writer::write_edge`] / [`Writer::update_edge`] / [`Writer::tombstone_edge`]
//!   / [`Writer::read_edge`] — edge slot primitives.
//! * [`Writer::commit`] — mark a transaction boundary + fsync WAL.
//! * [`Writer::msync_zones`] — force mmap flush (end-of-checkpoint only).
//!
//! The full property / string / cognitive write paths are wired in T4 on top
//! of these primitives.

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::{SubstrateFile, Zone, ZoneFile};
use crate::hilbert::{
    compute_hilbert_permutation_page_aligned, hilbert_key_from_features, zorder_key_from_features,
};
use crate::meta::meta_flags;
use crate::record::{EdgeRecord, NODES_PER_PAGE, NodeRecord, U48};
use crate::slot_remap::{self, RemapContext, SlotRemapRegistry};
use crate::wal::{WalPayload, WalRecord};
use crate::wal_io::{SyncMode, WalWriter};
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;

/// High-level write-through primitive for a substrate.
///
/// The writer keeps a single [`WalWriter`] and lazily materializes a
/// [`ZoneFile`] handle per zone. It grows zones exponentially (×1.5) to
/// limit the number of remaps during bulk inserts.
pub struct Writer {
    substrate: Arc<Mutex<SubstrateFile>>,
    wal: Arc<WalWriter>,
    /// Cache de handles de zone, en `RwLock` et NON en `Mutex`.
    ///
    /// (2026-08-07) Le `Mutex` sérialisait TOUTES les lectures de nœuds et
    /// d'arêtes : `read_node` / `read_edge` passent par `with_*_zone`, qui
    /// prenait un verrou EXCLUSIF alors qu'une lecture n'a besoin que de
    /// `&ZoneFile` (`as_slice(&self)`). C'était l'écriture qui imposait son
    /// contrat de verrouillage à la lecture.
    ///
    /// Mesuré avant correctif sur la base `agent/system` : débit plafonné à
    /// ~21 req/s quelle que soit la concurrence, 264 requêtes en file qui ne
    /// se résorbaient jamais, 35 workers tokio sur 40 endormis en
    /// `futex_do_wait`, et une part d'attente de 66 à 99 % selon les bases.
    /// Le MVCC du moteur est réel, mais il ne survivait pas à la descente
    /// vers le stockage.
    ///
    /// Les écritures gardent un accès exclusif via `.write()` — seules les
    /// lectures pures (`with_node_zone_read` / `with_edge_zone_read`)
    /// prennent `.read()` et peuvent courir ensemble.
    zones: RwLock<ZoneCache>,
    /// Growth policy: minimum bytes of headroom kept in each zone.
    min_headroom: u64,
    /// Chantier SlotRemappable — registre des side-cars slot-indexés
    /// enrôlés par le `SubstrateStore` (veccol, blobcol, degrés). Invoqué
    /// sous verrou writer par `bulk_sort_by_hilbert` /
    /// `compact_community`, APRÈS la durabilité du record WAL et AVANT le
    /// flip de l'epoch/flag. Voir `crate::slot_remap`.
    remap_registry: SlotRemapRegistry,
    /// (2026-09-06, fix CONSTAT-REMAP-PROPS-DETACHEMENT) Dernière permutation
    /// de slots `old→new` appliquée par `bulk_sort_by_hilbert`. Consommée par
    /// `SubstrateStore` juste après le sort pour rafraîchir les owner stamps
    /// (`header.node_id`) des chaînes PropsZone v2 déplacées — sinon la garde
    /// d'append détache la chaîne au prochain append (props pré-remap
    /// invisibles : perte silencieuse, repro `une_prop_ecrite_apres_un_remap_
    /// de_slot_ne_perd_pas_la_chaine`). `None` = jamais trié.
    derniere_permutation_remap: parking_lot::RwLock<Option<std::sync::Arc<Vec<u32>>>>,
    /// Offset WAL couvert par le dernier flush durable (chantier
    /// SlotRemappable). Voir [`Writer::require_empty_replay_window`].
    wal_baseline: std::sync::atomic::AtomicU64,
    /// Index d'unicité (kind, type_id, key_hash) → ids des simplexes
    /// VIVANTS (I2, spec 271180c3). Reconstruit paresseusement au premier
    /// upsert par balayage du répertoire — l'ouverture ne paie rien.
    /// ORDRE DE VERROUILLAGE : toujours `zones.write()` PUIS ce mutex.
    simplex_index: Mutex<Option<crate::simplex_zone::SimplexKeyIndex>>,
    /// Cache de la config des passes simpliciales (T5) : `None` = jamais
    /// lue ; `Some(x)` = valeur de la zone. Invalidé (réécrit) par
    /// [`Writer::set_simplex_passes_config`]. Évite l'aller-retour zone à
    /// CHAQUE mutation d'arête sur le chemin du commit — le hook de
    /// maintenance ne paie qu'un mutex non contendu quand la couche
    /// simpliciale n'est pas configurée. Mutex FEUILLE : jamais tenu en
    /// travers d'un verrou de zone.
    simplex_passes_cache: Mutex<Option<Option<crate::simplex_passes::SimplexPassesConfig>>>,
}

#[derive(Default)]
struct ZoneCache {
    nodes: Option<ZoneFile>,
    edges: Option<ZoneFile>,
    engram_members: Option<ZoneFile>,
    engram_bitset: Option<ZoneFile>,
    /// Zones simpliciales (T4, décision 2d461b89) — toujours manipulées
    /// ENSEMBLE sous le même verrou d'écriture : répertoire + heap des
    /// hyperarêtes + incidence + pages de postings.
    simplex_dir: Option<ZoneFile>,
    simplex_heap: Option<ZoneFile>,
    simplex_incidence: Option<ZoneFile>,
    simplex_postings: Option<ZoneFile>,
    /// ── DIRTY-FLAG de `msync_zones` (taille changée) ──
    ///
    /// `remap_count` de chaque zone au moment du dernier `fsync`. Il est
    /// incrémenté par `grow_to` / `shrink_to` / `rafraichir` — les trois
    /// seules opérations qui changent la TAILLE du fichier. Quand il diffère
    /// de la valeur courante, la zone a été ÉTENDUE (ou rétrécie) depuis le
    /// dernier fsync : la métadonnée de taille doit être rendue durable par un
    /// `fsync`. Sinon, `msync(MS_SYNC)` suffit — les pages de données sont
    /// déjà poussées sur disque, et l'ancien double flush (msync PUIS fsync
    /// inconditionnel) était du travail redondant payé à chaque flush.
    nodes_fsync_rc: std::sync::atomic::AtomicU64,
    edges_fsync_rc: std::sync::atomic::AtomicU64,
    engram_members_fsync_rc: std::sync::atomic::AtomicU64,
    engram_bitset_fsync_rc: std::sync::atomic::AtomicU64,
    simplex_dir_fsync_rc: std::sync::atomic::AtomicU64,
    simplex_heap_fsync_rc: std::sync::atomic::AtomicU64,
    simplex_incidence_fsync_rc: std::sync::atomic::AtomicU64,
    simplex_postings_fsync_rc: std::sync::atomic::AtomicU64,
}

/// Vue exclusive sur les quatre zones simpliciales, ouvertes paresseusement
/// et empruntées ensemble le temps d'une section critique.
pub(crate) struct SimplexZones<'a> {
    pub dir: &'a mut ZoneFile,
    pub heap: &'a mut ZoneFile,
    pub incidence: &'a mut ZoneFile,
    pub postings: &'a mut ZoneFile,
}

impl Writer {
    /// Construct a writer. The WAL is opened at `substrate.wal_path()`.
    pub fn new(substrate: SubstrateFile, sync_mode: SyncMode) -> SubstrateResult<Self> {
        // ── L'ANCRE DE REPLI, PLUS LE « PROCHAIN LSN » (2026-10-06) ─────────
        // `last_wal_offset` n'est écrit que par le checkpoint, qui le remet à 0
        // (`checkpoint.rs`) : il valait donc 0 en pratique, et TOUT processus
        // repartait de LSN 1. L'ancre durable vit désormais dans le side-car
        // `substrate.wal.lsn`, à côté du WAL, et c'est ELLE qui fait foi — y
        // compris entre processus et à travers les checkpoints. La valeur du
        // méta ne sert plus que de REPLI, pour une base antérieure à l'ancre
        // (side-car absent) : `0 + 1 = 1` y reproduit exactement l'ancien
        // amorçage, donc aucune base existante ne change de LSN.
        let ancre_de_repli = substrate.meta_header().last_wal_offset.saturating_add(1);
        // ── SUIS-JE LE SEUL ÉCRIVAIN ? (axe APPEND, 2026-10-06) ─────────────
        // Le verrou du RÉPERTOIRE le dit : `SubstrateFile::open` prend un verrou
        // EXCLUSIF — aucun autre processus ne peut ouvrir ce store, donc aucune
        // coordination d'append n'est due, et le chemin chaud reste intact.
        // `open_shared_write` / `open_read_only` prennent un verrou PARTAGÉ, que
        // deux écrivains peuvent tenir simultanément : là, l'append doit relire
        // la fin RÉELLE du fichier, sinon deux écrivains estampillent le même LSN
        // (la position devient une prédiction). On le DÉCLARE au WAL, qui choisit
        // son chemin en conséquence.
        // Une ouverture en LECTURE SEULE n'ajoute jamais : aucune coordination
        // d'append (donc aucun registre `<wal>.reg` créé), et un journal ouvert
        // sans droit d'écriture.
        let coordination = if substrate.est_ecriture_partagee() && !substrate.est_lecture_seule() {
            crate::wal_io::CoordinationAppend::OAppend
        } else {
            crate::wal_io::CoordinationAppend::Aucune
        };
        // (2026-09-17) En mode mémoire il n'y a pas de fichier WAL à ouvrir :
        // le journal vit dans un tampon (voir `WalWriter::open_memory`).
        let wal = if substrate.is_memory() {
            Arc::new(WalWriter::open_memory(sync_mode, ancre_de_repli)?)
        } else if substrate.est_lecture_seule() {
            let wal_path = substrate.wal_path();
            Arc::new(WalWriter::open_lecture_seule(
                &wal_path,
                sync_mode,
                ancre_de_repli,
            )?)
        } else {
            let wal_path = substrate.wal_path();
            Arc::new(WalWriter::open(&wal_path, sync_mode, ancre_de_repli)?)
        };
        wal.coordination(coordination);
        Ok(Self {
            substrate: Arc::new(Mutex::new(substrate)),
            wal,
            zones: RwLock::new(ZoneCache::default()),
            min_headroom: 1 << 20, // 1 MiB default
            remap_registry: SlotRemapRegistry::new(),
            derniere_permutation_remap: parking_lot::RwLock::new(None),
            wal_baseline: std::sync::atomic::AtomicU64::new(0),
            simplex_index: Mutex::new(None),
            simplex_passes_cache: Mutex::new(None),
        })
    }

    /// Access the shared [`SubstrateFile`] behind this writer.
    pub fn substrate(&self) -> Arc<Mutex<SubstrateFile>> {
        self.substrate.clone()
    }

    /// Registre des side-cars slot-indexés enrôlés dans les
    /// repermutations (chantier SlotRemappable). Le `SubstrateStore` y
    /// enrôle ses registres ouverts à l'open ; tout NOUVEAU side-car
    /// slot-indexé DOIT s'y enrôler avant tout câblage du warden.
    pub fn remap_registry(&self) -> SlotRemapRegistry {
        self.remap_registry.clone()
    }

    /// Copie de la dernière permutation `old→new` appliquée par le dernier
    /// `bulk_sort_by_hilbert` (`None` si aucun tri n'a eu lieu). Utilisée par
    /// `SubstrateStore` pour rafraîchir les owner stamps des chaînes
    /// PropsZone v2 déplacées (fix 2026-09-06, CONSTAT-REMAP-PROPS).
    pub fn derniere_permutation_remap(&self) -> Option<std::sync::Arc<Vec<u32>>> {
        self.derniere_permutation_remap.read().clone()
    }

    /// Access the underlying WAL writer (for advanced callers — group-commit
    /// timers, checkpoint orchestrator).
    pub fn wal(&self) -> Arc<WalWriter> {
        self.wal.clone()
    }

    /// Write a [`NodeRecord`] at `idx` and log a `NodeInsert` record.
    ///
    /// Protocol:
    /// 1. append `WalPayload::NodeInsert` to WAL (not yet committed),
    /// 2. mutate the mmap,
    /// 3. caller must call [`Writer::commit`] to seal the transaction.
    ///    BULK WRITE zone-direct (RFC §16.1, ingestion native) : écrit les zones
    ///    Nodes/Edges par blocs puis chaîne les listes d'adjacence en UNE passe
    ///    ascendante (invariant `next_from`/`next_to` → arête d'id INFÉRIEUR,
    ///    miroir de `create_edge`) et pose `NodeRecord.first_edge_off` = tête
    ///    (dernier id) de chaque source. PAS de WAL, PAS de degrés — l'appelant
    ///    (store) synchronise compteurs et sidecars. Store supposé NEUF (slots
    ///    1..=n, 1..=m).
    pub(crate) fn bulk_write(
        &self,
        node_bitsets: &[u64],
        edges: &[(u32, u32, u16)],
    ) -> SubstrateResult<(u32, u64)> {
        let n = node_bitsets.len();
        let m = edges.len() as u64;
        if n == 0 {
            return Ok((0, m));
        }
        // Zone Nodes : records contigus (slot 1..=n), slot 0 = sentinelle.
        self.with_node_zone(|zf| {
            let total = (n as u64 + 1) * NodeRecord::SIZE as u64;
            ensure_room(zf, total as usize, self.min_headroom)?;
            let bytes = zf.as_slice_mut();
            for (i, &bits) in node_bitsets.iter().enumerate() {
                let slot = i as u64 + 1;
                let rec = NodeRecord {
                    label_bitset: bits,
                    // (jalon 1) l'ecriture en masse est une ecriture reelle.
                    flags: crate::record::node_flags::ALLOCATED,
                    ..NodeRecord::default()
                };
                let off = (slot * NodeRecord::SIZE as u64) as usize;
                bytes[off..off + NodeRecord::SIZE].copy_from_slice(bytemuck::bytes_of(&rec));
            }
            Ok(())
        })?;
        // Zone Edges : slots 1..=m — chaînage FUSIONNÉ (une seule passe :
        // next_from/next_to posés à l'écriture, têtes mémorisées).
        if m > 0 {
            let cap = n + 1;
            let mut last_from = vec![0u64; cap];
            let mut heads_from = vec![0u64; cap];
            let mut last_to = vec![0u64; cap];
            self.with_edge_zone(|zf| {
                let total = (m + 1) * EdgeRecord::SIZE as u64;
                ensure_room(zf, total as usize, self.min_headroom)?;
                let bytes = zf.as_slice_mut();
                for (i, &(src, dst, ty)) in edges.iter().enumerate() {
                    let id = i as u64 + 1;
                    let s = src as usize;
                    let d = dst as usize;
                    if s >= cap || d >= cap {
                        return Err(SubstrateError::Internal(
                            "bulk_write: edge endpoint hors bornes (slots 1..=n)".into(),
                        ));
                    }
                    let rec = EdgeRecord {
                        src,
                        dst,
                        edge_type: ty,
                        next_from: if last_from[s] != 0 {
                            U48::from_u64(last_from[s] * EdgeRecord::SIZE as u64)
                        } else {
                            U48::ZERO
                        },
                        next_to: if last_to[d] != 0 {
                            U48::from_u64(last_to[d] * EdgeRecord::SIZE as u64)
                        } else {
                            U48::ZERO
                        },
                        ..EdgeRecord::default()
                    };
                    let off = (id * EdgeRecord::SIZE as u64) as usize;
                    bytes[off..off + EdgeRecord::SIZE].copy_from_slice(bytemuck::bytes_of(&rec));
                    last_from[s] = id;
                    heads_from[s] = id;
                    last_to[d] = id;
                }
                Ok(())
            })?;
            // Têtes dans NodeRecord.first_edge_off (slot 1..=n).
            self.with_node_zone(|zf| {
                let bytes = zf.as_slice_mut();
                for slot in 1..=n as u64 {
                    let off = (slot * NodeRecord::SIZE as u64) as usize;
                    let mut rec: NodeRecord =
                        *bytemuck::from_bytes(&bytes[off..off + NodeRecord::SIZE]);
                    rec.first_edge_off = if heads_from[slot as usize] != 0 {
                        U48::from_u64(heads_from[slot as usize] * EdgeRecord::SIZE as u64)
                    } else {
                        U48::ZERO
                    };
                    bytes[off..off + NodeRecord::SIZE].copy_from_slice(bytemuck::bytes_of(&rec));
                }
                Ok(())
            })?;
        }
        Ok((n as u32, m))
    }

    /// (RFC zorder-superposition, chantier 3) Marque une MUTATION de
    /// features : incrémente `feature_epoch` du substrate. Coût : un lock
    /// court + un fetch_add — les chemins d'écriture verrouillent déjà le
    /// substrate.
    fn note_feature_mutation(&self) {
        let sub = self.substrate.lock();
        sub.feature_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    #[tracing::instrument(level = "trace", skip(self, node))]
    pub fn write_node(&self, idx: u32, node: NodeRecord) -> SubstrateResult<()> {
        self.ecrire_node(idx, node, None)
    }

    /// Comme [`Writer::write_node`], mais la déclaration `precede` (un record
    /// écrit AVANT le `NodeInsert`, p. ex. `TxNoeudEnAttente`) part dans le
    /// MÊME `write` que le `NodeInsert` : un appel système de moins, ordre
    /// physique préservé (obrain#120 pt 4).
    pub fn write_node_precede(
        &self,
        idx: u32,
        node: NodeRecord,
        precede: WalPayload,
    ) -> SubstrateResult<()> {
        self.ecrire_node(idx, node, Some(precede))
    }

    fn ecrire_node(
        &self,
        idx: u32,
        node: NodeRecord,
        precede: Option<WalPayload>,
    ) -> SubstrateResult<()> {
        // (jalon 1) TOUT record reellement ecrit porte ALLOCATED — pose ici,
        // au seul point d'ecriture, jamais laisse au choix de l'appelant.
        let node = NodeRecord {
            flags: node.flags | crate::record::node_flags::ALLOCATED,
            ..node
        };
        self.note_feature_mutation();
        // (1) WAL first. (jalon 2) La trame V3 porte le tenant du record —
        // le rejeu restaure l'appartenance a l'identique. Le defaut public
        // (0) vient du record lui-meme (`NodeRecord::default()`), pas d'ici.
        let rec = WalRecord {
            lsn: 0, // assigned by WalWriter::append
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::NodeInsertV3 {
                node_id: idx,
                label_bitset: node.label_bitset,
                tenant_id: node.tenant_id,
            },
        };
        match precede {
            None => {
                self.wal.append(rec)?;
            }
            Some(payload) => {
                let decl = WalRecord {
                    lsn: 0,
                    timestamp: unix_micros(),
                    flags: 0,
                    payload,
                };
                self.wal.append_batch([decl, rec])?;
            }
        }

        // (2) mmap mutation.
        self.with_node_zone(|zf| {
            let offset = idx as usize * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, self.min_headroom)?;
            let bytes = bytemuck::bytes_of(&node);
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE].copy_from_slice(bytes);
            Ok(())
        })
    }

    /// Rewrite an existing [`NodeRecord`] at `idx` and log a `NodeUpdate`.
    ///
    /// Unlike [`Writer::write_node`] this logs the richer `NodeUpdate` payload
    /// that carries every mutable cognitive field — replay restores the slot
    /// exactly as it was at commit time.
    #[tracing::instrument(level = "trace", skip(self, node))]
    pub fn update_node(&self, idx: u32, node: NodeRecord) -> SubstrateResult<()> {
        // (jalon 1) meme regle que write_node — ALLOCATED avant le WAL,
        // pour que le payload NodeUpdate rejoue le bit a l'identique.
        let node = NodeRecord {
            flags: node.flags | crate::record::node_flags::ALLOCATED,
            ..node
        };
        // (jalon 2) Trame V3 : le tenant voyage avec les autres champs
        // mutables — le rejeu restaure le slot exactement, appartenance
        // comprise.
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::NodeUpdateV3 {
                node_id: idx,
                label_bitset: node.label_bitset,
                energy: node.energy,
                scar_util_affinity: node.scar_util_affinity,
                centrality_cached: node.centrality_cached,
                flags: node.flags,
                community_id: node.community_id,
                tenant_id: node.tenant_id,
            },
        };
        self.wal.append(rec)?;

        self.with_node_zone(|zf| {
            let offset = idx as usize * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, self.min_headroom)?;
            let bytes = bytemuck::bytes_of(&node);
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE].copy_from_slice(bytes);
            Ok(())
        })
    }

    /// Update only the `first_prop_off` head pointer of the node at `idx`
    /// (T17c Step 3b.2). Logs a [`WalPayload::NodePropHeadUpdate`] before
    /// mutating the 6-byte `first_prop_off` slice at byte offset 14 within
    /// the record — the rest of the `NodeRecord` is left untouched.
    ///
    /// This is the WAL-first primitive used by `SubstrateStore` when the v2
    /// props zone re-anchors a node's property chain (insert at head of
    /// linked list). Replay re-applies the same 6 bytes idempotently.
    ///
    /// Silently no-ops if the slot is beyond the current zone length — the
    /// caller has asked to update the head pointer of a node that does not
    /// yet exist in the nodes zone, which is always a bug, but we prefer to
    /// let the WAL-only record stand rather than panic (replay will rebuild
    /// the slot when the matching `NodeInsert` arrives).
    #[tracing::instrument(level = "trace", skip(self))]
    /// (2026-09-16, T27) Pose les têtes de chaîne de propriétés d'une
    /// ingestion EN LOT, en une passe sur la zone nœuds et SANS WAL par
    /// nœud — même contrat que `bulk_write` : base neuve, l'appelant
    /// checkpoint à la fin. Le chemin unitaire journalise une
    /// `NodePropHeadUpdate` par PROPRIÉTÉ, soit ≈ 12 M trames sur le noyau
    /// Linux pour un état que le checkpoint rend durable de toute façon.
    pub fn poser_tetes_props_en_lot(&self, tetes: &[(u32, U48)]) -> SubstrateResult<()> {
        if tetes.is_empty() {
            return Ok(());
        }
        self.with_node_zone(|zf| {
            let octets = zf.as_slice_mut();
            for &(idx, tete) in tetes {
                let champ = idx as usize * NodeRecord::SIZE + 14; // first_prop_off
                if champ + 6 > octets.len() {
                    return Err(SubstrateError::Internal(format!(
                        "poser_tetes_props_en_lot : slot {idx} hors de la zone nœuds"
                    )));
                }
                octets[champ..champ + 6].copy_from_slice(&tete.0);
            }
            Ok(())
        })
    }

    pub fn update_node_first_prop_off(&self, idx: u32, new_head: U48) -> SubstrateResult<()> {
        // (1) WAL first.
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::NodePropHeadUpdate {
                node_id: idx,
                first_prop_off: new_head.to_u64(),
            },
        };
        self.wal.append(rec)?;

        // (2) mmap mutation — 6 bytes at offset (idx * NodeRecord::SIZE + 14).
        // (jalon 2 : le stride est 40 ; l'offset 14 de first_prop_off n'a
        // pas bouge, le tenant s'ajoute en queue de record.)
        self.with_node_zone(|zf| {
            let record_offset = idx as usize * NodeRecord::SIZE;
            let field_offset = record_offset + 14; // first_prop_off byte offset inside NodeRecord
            if field_offset + 6 > zf.as_slice().len() {
                // Slot not materialised yet — leave the WAL record as the
                // authoritative post-state; the matching NodeInsert (or a
                // later NodeUpdate) will materialise the slot, and this
                // NodePropHeadUpdate record will be re-applied at replay in
                // LSN order to restore the head pointer.
                return Ok(());
            }
            zf.as_slice_mut()[field_offset..field_offset + 6].copy_from_slice(&new_head.0);
            Ok(())
        })
    }

    /// GARANTIE MEMOIRE (2026-07-25) : append generique d un payload au
    /// WAL — meme construction d enregistrement que
    /// update_node_first_prop_off (lsn/timestamp/flags identiques). Utilise
    /// par SubstrateStore pour journaliser les proprietes scalaires inline
    /// (PropSetInline & co) au moment exact de l ecriture sidecar.
    pub(crate) fn append_payload(&self, payload: WalPayload) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload,
        };
        self.wal.append(rec)?;
        Ok(())
    }

    /// ── PROPRIÉTÉ SCALAIRE INLINE, SANS ALLOCATION PAR RECORD (pièce 2) ─────
    ///
    /// Le chemin d'écriture de propriété du store passait par
    /// `append_payload(WalPayload::PropSetInline { key: key.to_string(),
    /// value_bincode: bytes })` : une `String` ET un `Vec<u8>` alloués par record
    /// pour des données que l'appelant possédait déjà (`&str` + `&[u8]`), soit
    /// C1 = 46,4 ns/record mesurés. Ici rien n'est cloné : le payload est encodé
    /// EMPRUNTÉ, et les octets sur disque sont identiques
    /// (voir `WalPayload`/`PropSetInlineRef` dans `wal.rs`).
    pub(crate) fn append_prop_set_inline(
        &self,
        node_id: u32,
        key: &str,
        value_bincode: &[u8],
    ) -> SubstrateResult<()> {
        self.wal
            .append_prop_set_inline(node_id, key, value_bincode, unix_micros())?;
        Ok(())
    }

    /// Tombstone the node at `idx`: logs `NodeDelete` and flips the
    /// [`node_flags::TOMBSTONED`](crate::record::node_flags::TOMBSTONED) bit
    /// in place without touching the rest of the record.
    ///
    /// Silently no-ops if the slot is beyond the current zone length (replay
    /// may request a delete for a node that was never materialised past a
    /// crash — the tombstone is then implicit).
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn tombstone_node(&self, idx: u32) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::NodeDelete { node_id: idx },
        };
        self.wal.append(rec)?;

        self.with_node_zone(|zf| {
            let offset = idx as usize * NodeRecord::SIZE;
            if offset + NodeRecord::SIZE > zf.as_slice().len() {
                return Ok(());
            }
            // Read-modify-write with bytemuck's safe Pod API.
            let cur: NodeRecord = {
                let bytes = &zf.as_slice()[offset..offset + NodeRecord::SIZE];
                *bytemuck::from_bytes::<NodeRecord>(bytes)
            };
            let mut updated = cur;
            updated.set_tombstoned();
            let bytes = bytemuck::bytes_of(&updated);
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE].copy_from_slice(bytes);
            Ok(())
        })
    }

    /// Inverse exact de [`Self::tombstone_node`] : efface le bit
    /// [`node_flags::TOMBSTONED`](crate::record::node_flags::TOMBSTONED) sans
    /// toucher au reste de l'enregistrement.
    ///
    /// (2026-08-14) Sert a annuler une suppression au sein d'une transaction.
    /// Le tombstone ne detruit rien — il leve un drapeau, et le
    /// `label_bitset`, les chainages et les offsets de proprietes restent
    /// intacts dans le slot. La resurrection est donc EXACTE, pas une
    /// reconstruction approchee : le noeud retrouve son identite, ses aretes
    /// et sa place dans les chaines.
    ///
    /// AUCUNE VARIANTE DE WAL N'A ETE AJOUTEE, et c'est ce qui rend l'affaire
    /// abordable : `WalPayload::NodeUpdate` porte deja un champ `flags`. On
    /// journalise donc l'enregistrement complet avec le drapeau efface, ce que
    /// le rejeu apres panne applique naturellement. Pas de changement de
    /// format, pas de version a faire evoluer, pas de fichier existant a
    /// migrer.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn revive_node(&self, idx: u32) -> SubstrateResult<()> {
        let Some(cur) = self.read_node(idx)? else {
            return Ok(());
        };
        let mut updated = cur;
        updated.flags &= !crate::record::node_flags::TOMBSTONED;

        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::NodeUpdate {
                node_id: idx,
                label_bitset: updated.label_bitset,
                energy: updated.energy,
                scar_util_affinity: updated.scar_util_affinity,
                centrality_cached: updated.centrality_cached,
                flags: updated.flags,
                community_id: updated.community_id,
            },
        };
        self.wal.append(rec)?;

        self.with_node_zone(|zf| {
            let offset = idx as usize * NodeRecord::SIZE;
            if offset + NodeRecord::SIZE > zf.as_slice().len() {
                return Ok(());
            }
            let bytes = bytemuck::bytes_of(&updated);
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE].copy_from_slice(bytes);
            Ok(())
        })
    }

    /// Inverse exact de [`Self::tombstone_edge`]. Meme raisonnement que
    /// [`Self::revive_node`] : `WalPayload::EdgeUpdate` porte deja `flags`.
    ///
    /// ATTENTION : ceci ne remet PAS l'arete dans les chaines d'adjacence.
    /// `delete_edge` la desenfile avant de poser le tombstone ; le rechainage
    /// est la responsabilite de l'appelant, cote store, ou vivent les
    /// helpers de splice. Separer les deux evite de dupliquer ici une logique
    /// de chainage qui a deja son verrou et ses invariants ailleurs.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn revive_edge(&self, idx: u64) -> SubstrateResult<()> {
        let Some(cur) = self.read_edge(idx)? else {
            return Ok(());
        };
        let mut updated = cur;
        updated.flags &= !crate::record::edge_flags::TOMBSTONED;

        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::EdgeUpdate {
                edge_id: idx,
                weight_u16: updated.weight_u16,
                ricci_u8: updated.ricci_u8,
                flags: updated.flags,
                engram_tag: updated.engram_tag,
            },
        };
        self.wal.append(rec)?;

        self.with_edge_zone(|zf| {
            let offset = idx as usize * EdgeRecord::SIZE;
            if offset + EdgeRecord::SIZE > zf.as_slice().len() {
                return Ok(());
            }
            let bytes = bytemuck::bytes_of(&updated);
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE].copy_from_slice(bytes);
            Ok(())
        })
    }

    /// Read the [`NodeRecord`] at slot `idx`. Returns `None` if the slot is
    /// beyond the current zone length (never allocated).
    pub fn read_node(&self, idx: u32) -> SubstrateResult<Option<NodeRecord>> {
        // (2026-08-07) Lecture PARTAGÉE — voir `with_node_zone_read`. C'est
        // le chemin le plus chaud du moteur (`get_node`, tout `MATCH`) et il
        // prenait jusqu'ici un verrou exclusif.
        self.with_node_zone_read(|zf| {
            let offset = idx as usize * NodeRecord::SIZE;
            if offset + NodeRecord::SIZE > zf.as_slice().len() {
                return Ok(None);
            }
            let bytes = &zf.as_slice()[offset..offset + NodeRecord::SIZE];
            Ok(Some(*bytemuck::from_bytes::<NodeRecord>(bytes)))
        })
    }

    /// Parcourt les enregistrements de nœud des slots `[1, hw)` en ne prenant
    /// le verrou de zone qu'UNE SEULE FOIS pour toute la boucle.
    ///
    /// (2026-08-07) Les scans de label appelaient [`Writer::read_node`] slot
    /// par slot. Sur la base `agent` (216 000 slots), un seul `MATCH (n:X)`
    /// prenait donc 216 000 fois le verrou de zone. Mesuré avant correctif :
    /// 458 ms pour 10 000 slots, soit ~46 µs par nœud — pour lire 32 octets
    /// déjà présents en mémoire mappée. Extrapolé à la base entière, un scan
    /// complet coûtait une dizaine de secondes, dont l'écrasante majorité en
    /// acquisitions de verrou et non en lecture de données.
    ///
    /// `f` reçoit `(slot, &NodeRecord)` et rend `false` pour arrêter le
    /// parcours (utilisé par les scans à `LIMIT`).
    ///
    /// ATTENTION : `f` s'exécute AVEC le verrou de zone tenu en lecture. Elle
    /// ne doit ni écrire dans la zone, ni rappeler une méthode qui reprend
    /// `self.zones` — `parking_lot::RwLock` n'est pas réentrant.
    /// Variante BORNÉE de [`Writer::for_each_node_record`] : ne balaie que
    /// `debut..fin`, donc ne tient le verrou de zone que le temps de cette
    /// tranche.
    ///
    /// ── POURQUOI ELLE EXISTE (2026-08-19) ──
    ///
    /// `for_each_node_record` tient la lecture de zone pendant TOUT le
    /// balayage. La construction d index depuis PropsZone v2 l utilisait
    /// ainsi, et le chemin d écriture de propriété tient `props_zone` en
    /// ÉCRITURE tout en réclamant, imbriquée, l écriture de zone de nœuds
    /// (`update_node_first_prop_off`). Avec plusieurs lecteurs qui se
    /// relaient, la lecture de zone n était jamais libre : l écrivain
    /// n obtenait pas son verrou, et les lecteurs attendaient ensuite le
    /// `props_zone` qu il détenait. Pas un interblocage au sens strict —
    /// une famine — mais l effet observé est le même : un test croisant
    /// 4 lecteurs et 4 écrivains ne terminait pas en 120 s, alors que
    /// chaque famille seule s exécute en moins d une seconde.
    pub(crate) fn for_each_node_record_range<F>(
        &self,
        debut: u32,
        fin: u32,
        mut f: F,
    ) -> SubstrateResult<()>
    where
        F: FnMut(u32, &NodeRecord) -> bool,
    {
        self.with_node_zone_read(|zf| {
            let bytes = zf.as_slice();
            for slot in debut.max(1)..fin {
                let offset = slot as usize * NodeRecord::SIZE;
                if offset + NodeRecord::SIZE > bytes.len() {
                    break;
                }
                let rec =
                    bytemuck::from_bytes::<NodeRecord>(&bytes[offset..offset + NodeRecord::SIZE]);
                if !f(slot, rec) {
                    break;
                }
            }
            Ok(())
        })
    }

    pub(crate) fn for_each_node_record<F>(&self, hw: u32, mut f: F) -> SubstrateResult<()>
    where
        F: FnMut(u32, &NodeRecord) -> bool,
    {
        self.with_node_zone_read(|zf| {
            let bytes = zf.as_slice();
            for slot in 1..hw {
                let offset = slot as usize * NodeRecord::SIZE;
                if offset + NodeRecord::SIZE > bytes.len() {
                    break;
                }
                let rec =
                    bytemuck::from_bytes::<NodeRecord>(&bytes[offset..offset + NodeRecord::SIZE]);
                if !f(slot, rec) {
                    break;
                }
            }
            Ok(())
        })
    }

    // ═════════════════════════════════════════════════════════════════
    // VARIANTES INTERRUPTIBLES (2026-08-24, annulation coopérative)
    // ═════════════════════════════════════════════════════════════════
    //
    // POURQUOI DES VARIANTES ET PAS UN CONTRÔLE DANS LES ORIGINALES.
    //
    // Ces trois boucles sont le socle de TOUT le balayage du substrat —
    // scans de label, mais aussi reconstructions d'index et de compteurs
    // (`rebuild_degrees_from_scan`, `refresh_community_ranges`,
    // `batir_index_depuis_v2`). Une reconstruction TRONQUÉE ne rend pas un
    // résultat partiel à un appelant qui le jettera : elle PERSISTE un index
    // faux, que toutes les requêtes suivantes croiront. Le dégât survit à
    // l'annulation.
    //
    // Rendre les originales interruptibles ferait donc du défaut la
    // dangereuse : tout chemin oublié deviendrait tronquable. Le défaut
    // reste NON interruptible, et l'interruption s'OPTE-IN, chemin par
    // chemin, uniquement là où le résultat est consommé par une requête de
    // LECTURE et jeté avec elle.
    //
    // COÛT : une `Vigie` par appel, soit un `usize` de pile et, une
    // itération sur 1024, une lecture de thread-local + un `load(Relaxed)`.
    //
    // SÛRETÉ DES VERROUS : le `?` déroule la pile normalement ; la garde
    // RAII de `with_*_zone_read` (parking_lot) relâche le verrou de zone à
    // son `Drop`. Aucun verrou orphelin — et surtout, le point de contrôle
    // ne REPREND aucun verrou : `parking_lot::RwLock` n'est pas réentrant,
    // et un contrôle qui rappellerait `self.zones` interbloquerait ici.
    // Seul un `AtomicBool::load` est sûr sous ce verrou.

    /// Variante INTERRUPTIBLE de [`Writer::for_each_node_record`].
    ///
    /// Rend `Err(SubstrateError::Cancelled)` si le jeton du thread est armé.
    /// Réservée aux chemins de LECTURE dont le résultat meurt avec la requête.
    pub(crate) fn for_each_node_record_annulable<F>(&self, hw: u32, mut f: F) -> SubstrateResult<()>
    where
        F: FnMut(u32, &NodeRecord) -> bool,
    {
        let mut vigie = obrain_common::utils::cancel::Vigie::new();
        // (2026-09-11) COMPTAGE DES SLOTS REELLEMENT LUS. Cumul LOCAL, verse
        // dans l'atomique UNE FOIS a la sortie : le chemin chaud ne paie
        // aucun trafic atomique par slot. C'est l'instrument de preuve du
        // plafond sans etiquette — un nombre comptable et deterministe, la
        // ou le chronometre de cette machine ne vaut rien.
        let mut lus: u64 = 0;
        let issue = self.with_node_zone_read(|zf| {
            let bytes = zf.as_slice();
            for slot in 1..hw {
                if vigie.tick_bool() {
                    return Err(crate::error::SubstrateError::Cancelled);
                }
                let offset = slot as usize * NodeRecord::SIZE;
                if offset + NodeRecord::SIZE > bytes.len() {
                    break;
                }
                let rec =
                    bytemuck::from_bytes::<NodeRecord>(&bytes[offset..offset + NodeRecord::SIZE]);
                lus += 1;
                if !f(slot, rec) {
                    break;
                }
            }
            Ok(())
        });
        crate::compteurs::ajouter_slots_noeuds_lus(lus);
        issue
    }

    /// Variante INTERRUPTIBLE de [`Writer::for_each_node_record_range`].
    pub(crate) fn for_each_node_record_range_annulable<F>(
        &self,
        debut: u32,
        fin: u32,
        mut f: F,
    ) -> SubstrateResult<()>
    where
        F: FnMut(u32, &NodeRecord) -> bool,
    {
        let mut vigie = obrain_common::utils::cancel::Vigie::new();
        self.with_node_zone_read(|zf| {
            let bytes = zf.as_slice();
            for slot in debut.max(1)..fin {
                if vigie.tick_bool() {
                    return Err(crate::error::SubstrateError::Cancelled);
                }
                let offset = slot as usize * NodeRecord::SIZE;
                if offset + NodeRecord::SIZE > bytes.len() {
                    break;
                }
                let rec =
                    bytemuck::from_bytes::<NodeRecord>(&bytes[offset..offset + NodeRecord::SIZE]);
                if !f(slot, rec) {
                    break;
                }
            }
            Ok(())
        })
    }

    /// Variante INTERRUPTIBLE de [`Writer::for_each_edge_record`].
    pub(crate) fn for_each_edge_record_annulable<F>(&self, hw: u64, mut f: F) -> SubstrateResult<()>
    where
        F: FnMut(u64, &EdgeRecord) -> bool,
    {
        let mut vigie = obrain_common::utils::cancel::Vigie::new();
        self.with_edge_zone_read(|zf| {
            let bytes = zf.as_slice();
            for slot in 1..hw {
                if vigie.tick_bool() {
                    return Err(crate::error::SubstrateError::Cancelled);
                }
                let offset = slot as usize * EdgeRecord::SIZE;
                if offset + EdgeRecord::SIZE > bytes.len() {
                    break;
                }
                let rec =
                    bytemuck::from_bytes::<EdgeRecord>(&bytes[offset..offset + EdgeRecord::SIZE]);
                if !f(slot, rec) {
                    break;
                }
            }
            Ok(())
        })
    }

    /// Équivalent de [`Writer::for_each_node_record`] pour les arêtes.
    ///
    /// ATTENTION : mêmes contraintes de réentrance.
    pub(crate) fn for_each_edge_record<F>(&self, hw: u64, mut f: F) -> SubstrateResult<()>
    where
        F: FnMut(u64, &EdgeRecord) -> bool,
    {
        self.with_edge_zone_read(|zf| {
            let bytes = zf.as_slice();
            for slot in 1..hw {
                let offset = slot as usize * EdgeRecord::SIZE;
                if offset + EdgeRecord::SIZE > bytes.len() {
                    break;
                }
                let rec =
                    bytemuck::from_bytes::<EdgeRecord>(&bytes[offset..offset + EdgeRecord::SIZE]);
                if !f(slot, rec) {
                    break;
                }
            }
            Ok(())
        })
    }

    /// Donne accès à la zone des nœuds sous UN SEUL verrou, pour les accès
    /// ALÉATOIRES à une liste de slots connue d'avance — typiquement les
    /// tests de vivacité d'un lot d'identifiants.
    ///
    /// (2026-08-07) `get_node_property` teste la vivacité via
    /// `is_live_on_disk`, qui fait un `read_node` complet — donc une
    /// acquisition de verrou — pour ne consulter qu'un bit. En lecture
    /// unitaire c'est incompressible ; en lot, ça se mutualise.
    ///
    /// Mêmes contraintes de réentrance que [`Writer::with_edge_zone_records`].
    pub(crate) fn with_node_zone_records<R>(
        &self,
        f: impl FnOnce(&mut dyn FnMut(u32) -> Option<NodeRecord>) -> R,
    ) -> SubstrateResult<R> {
        self.with_node_zone_read(|zf| {
            let bytes = zf.as_slice();
            let mut lire = |idx: u32| -> Option<NodeRecord> {
                let offset = idx as usize * NodeRecord::SIZE;
                if offset + NodeRecord::SIZE > bytes.len() {
                    return None;
                }
                Some(*bytemuck::from_bytes::<NodeRecord>(
                    &bytes[offset..offset + NodeRecord::SIZE],
                ))
            };
            Ok(f(&mut lire))
        })
    }

    /// Donne accès à la zone des arêtes sous UN SEUL verrou, pour les
    /// parcours à accès ALÉATOIRE — les chaînes chaînées `next_from` /
    /// `next_to`, que `for_each_edge_record` ne sait pas suivre puisqu'il
    /// balaie les slots dans l'ordre.
    ///
    /// (2026-08-07) `walk_outgoing_chain` / `walk_incoming_chain` — le chemin
    /// de toute traversée `-[:TYPE]->` — appelaient `read_edge` par maillon,
    /// donc un verrou par arête parcourue.
    ///
    /// La closure reçoit un lecteur de slot. Elle NE DOIT PAS appeler de code
    /// utilisateur ni reprendre `self.zones` : les appelants collectent la
    /// chaîne ici, et n'invoquent leur propre closure qu'APRÈS la sortie,
    /// verrou relâché. C'est délibérément plus strict que nécessaire — les
    /// appelants actuels sont tous inoffensifs, mais faire dépendre l'absence
    /// d'interblocage de la bonne conduite des futurs appelants serait un
    /// piège.
    pub(crate) fn with_edge_zone_records<R>(
        &self,
        f: impl FnOnce(&mut dyn FnMut(u64) -> Option<EdgeRecord>) -> R,
    ) -> SubstrateResult<R> {
        self.with_edge_zone_read(|zf| {
            let bytes = zf.as_slice();
            let mut lire = |idx: u64| -> Option<EdgeRecord> {
                let offset = idx as usize * EdgeRecord::SIZE;
                if offset + EdgeRecord::SIZE > bytes.len() {
                    return None;
                }
                Some(*bytemuck::from_bytes::<EdgeRecord>(
                    &bytes[offset..offset + EdgeRecord::SIZE],
                ))
            };
            Ok(f(&mut lire))
        })
    }

    /// Write an [`EdgeRecord`] at `idx` and log an `EdgeInsert` record.
    #[tracing::instrument(level = "trace", skip(self, edge))]
    pub fn write_edge(&self, idx: u64, edge: EdgeRecord) -> SubstrateResult<()> {
        self.note_feature_mutation();
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::EdgeInsert {
                edge_id: idx,
                src: edge.src,
                dst: edge.dst,
                edge_type: edge.edge_type,
                weight_u16: edge.weight_u16,
            },
        };
        self.wal.append(rec)?;

        self.with_edge_zone(|zf| {
            let offset = idx as usize * EdgeRecord::SIZE;
            ensure_room(zf, offset + EdgeRecord::SIZE, self.min_headroom)?;
            let bytes = bytemuck::bytes_of(&edge);
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE].copy_from_slice(bytes);
            Ok(())
        })
    }

    /// Rewrite an existing [`EdgeRecord`] at `idx` and log an `EdgeUpdate`.
    ///
    /// Unlike [`Writer::write_edge`] this logs the richer `EdgeUpdate` payload
    /// that carries the mutable cognitive fields (weight, ricci, flags, engram
    /// tag). The chain pointers (`next_from`/`next_to`) and the immutable
    /// `src`/`dst`/`edge_type` are taken from the full `edge` argument —
    /// callers MUST read the record first, mutate, then pass the complete
    /// updated version.
    ///
    /// Replay restores the slot exactly as it was at commit time.
    #[tracing::instrument(level = "trace", skip(self, edge))]
    pub fn update_edge(&self, idx: u64, edge: EdgeRecord) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::EdgeUpdate {
                edge_id: idx,
                weight_u16: edge.weight_u16,
                ricci_u8: edge.ricci_u8,
                flags: edge.flags,
                engram_tag: edge.engram_tag,
            },
        };
        self.wal.append(rec)?;

        self.with_edge_zone(|zf| {
            let offset = idx as usize * EdgeRecord::SIZE;
            ensure_room(zf, offset + EdgeRecord::SIZE, self.min_headroom)?;
            let bytes = bytemuck::bytes_of(&edge);
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE].copy_from_slice(bytes);
            Ok(())
        })
    }

    /// Tombstone the edge at `idx`: logs `EdgeDelete` and flips the
    /// [`edge_flags::TOMBSTONED`](crate::record::edge_flags::TOMBSTONED) bit
    /// in place without touching the rest of the record.
    ///
    /// Silently no-ops if the slot is beyond the current zone length.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn tombstone_edge(&self, idx: u64) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::EdgeDelete { edge_id: idx },
        };
        self.wal.append(rec)?;

        self.with_edge_zone(|zf| {
            let offset = idx as usize * EdgeRecord::SIZE;
            if offset + EdgeRecord::SIZE > zf.as_slice().len() {
                return Ok(());
            }
            let cur: EdgeRecord = {
                let bytes = &zf.as_slice()[offset..offset + EdgeRecord::SIZE];
                *bytemuck::from_bytes::<EdgeRecord>(bytes)
            };
            let mut updated = cur;
            updated.flags |= crate::record::edge_flags::TOMBSTONED;
            let bytes = bytemuck::bytes_of(&updated);
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE].copy_from_slice(bytes);
            Ok(())
        })
    }

    /// Read the [`EdgeRecord`] at slot `idx`. Returns `None` if the slot is
    /// beyond the current zone length (never allocated).
    pub fn read_edge(&self, idx: u64) -> SubstrateResult<Option<EdgeRecord>> {
        // (2026-08-07) Lecture PARTAGÉE — voir `with_node_zone_read`.
        self.with_edge_zone_read(|zf| {
            let offset = idx as usize * EdgeRecord::SIZE;
            if offset + EdgeRecord::SIZE > zf.as_slice().len() {
                return Ok(None);
            }
            let bytes = &zf.as_slice()[offset..offset + EdgeRecord::SIZE];
            Ok(Some(*bytemuck::from_bytes::<EdgeRecord>(bytes)))
        })
    }

    /// Update only the `first_prop_off` head pointer of the edge at `idx`
    /// (T17f Step 3). Logs a [`WalPayload::EdgePropHeadUpdate`] before
    /// mutating the 6-byte `first_prop_off` slice at byte offset
    /// [`EdgeRecord::FIRST_PROP_OFF_BYTES`] (= 24) within the record — the
    /// rest of the `EdgeRecord` is left untouched.
    ///
    /// This is the WAL-first primitive used by `SubstrateStore` when the v2
    /// props zone re-anchors an edge's property chain (insert at head of
    /// linked list). Replay re-applies the same 6 bytes idempotently.
    ///
    /// Silently no-ops if the slot is beyond the current zone length — the
    /// caller has asked to update the head pointer of an edge that does not
    /// yet exist in the edges zone, which is always a bug, but we prefer to
    /// let the WAL-only record stand rather than panic (replay will rebuild
    /// the slot when the matching `EdgeInsert` arrives).
    ///
    /// Mirrors [`Writer::update_node_first_prop_off`] for edges — the only
    /// differences are the WAL payload variant, the record stride
    /// ([`EdgeRecord::SIZE`] vs [`NodeRecord::SIZE`]), and the field offset
    /// within the record (24 vs 14).
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn update_edge_first_prop_off(&self, idx: u32, new_head: U48) -> SubstrateResult<()> {
        // (1) WAL first.
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::EdgePropHeadUpdate {
                edge_idx: idx,
                first_prop_off: new_head.to_u64(),
            },
        };
        self.wal.append(rec)?;

        // (2) mmap mutation — 6 bytes at offset (idx * 36 + 24).
        self.with_edge_zone(|zf| {
            let record_offset = idx as usize * EdgeRecord::SIZE;
            let field_offset = record_offset + EdgeRecord::FIRST_PROP_OFF_BYTES;
            if field_offset + 6 > zf.as_slice().len() {
                // Slot not materialised yet — leave the WAL record as the
                // authoritative post-state; the matching EdgeInsert (or a
                // later EdgeUpdate) will materialise the slot, and this
                // EdgePropHeadUpdate record will be re-applied at replay in
                // LSN order to restore the head pointer.
                return Ok(());
            }
            zf.as_slice_mut()[field_offset..field_offset + 6].copy_from_slice(&new_head.0);
            Ok(())
        })
    }

    // ==================================================================
    // Cognitive column primitives (T6)
    // ==================================================================
    //
    // These methods write directly to the typed columns carried inside
    // `NodeRecord` / `EdgeRecord` — energy, scar/util/affinity, synapse
    // weight, centrality — using dedicated WAL payloads. They preserve
    // the "WAL-first, then mmap" invariant but avoid the full NodeUpdate
    // payload overhead for per-cognition mutations.
    //
    // All methods are idempotent under replay (payloads carry absolute
    // post-state, never deltas) except [`decay_all_energy`] whose semantics
    // are "multiply all live slots by factor_q16"; replay re-applies the
    // same multiplicative step in order (see `replay.rs`).

    /// Reinforce a single node's energy to `new_energy_u16` (Q1.15).
    ///
    /// WAL payload: [`WalPayload::EnergyReinforce`].
    ///
    /// Idempotent: replaying the same record writes the same u16 into the
    /// same slot.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn reinforce_energy(&self, idx: u32, new_energy_u16: u16) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::EnergyReinforce {
                node_id: idx,
                new_energy: new_energy_u16,
            },
        };
        self.wal.append(rec)?;

        self.with_node_zone(|zf| {
            let offset = idx as usize * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, self.min_headroom)?;
            let cur: NodeRecord = {
                let bytes = &zf.as_slice()[offset..offset + NodeRecord::SIZE];
                *bytemuck::from_bytes::<NodeRecord>(bytes)
            };
            let mut updated = cur;
            updated.energy = new_energy_u16;
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(())
        })
    }

    /// Apply an eager decay to every live node's energy column:
    /// `energy ← energy × factor / 65536` (Q0.16 multiplier).
    ///
    /// This is the "eager-periodic-batch" replacement for the previous
    /// lazy-per-read decay that cached a per-node `last_activated`
    /// timestamp. The Consolidator Thinker (T13) owns the scheduling.
    ///
    /// WAL payload: [`WalPayload::EnergyDecay`] (single compact record).
    ///
    /// `high_water` is the exclusive upper bound of live node slots (i.e.
    /// `SubstrateStore::slot_high_water()`). Tombstoned slots are skipped.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn decay_all_energy(&self, factor_q16: u16, high_water: u32) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::EnergyDecay { factor_q16 },
        };
        self.wal.append(rec)?;

        self.with_node_zone(|zf| {
            apply_energy_decay_to_zone(zf, factor_q16, high_water);
            Ok(())
        })
    }

    /// Overwrite the packed scar/utility/affinity byte-pair of a node.
    ///
    /// WAL payload: [`WalPayload::ScarUtilAffinitySet`].
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn set_scar_util_affinity(&self, idx: u32, packed: u16) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::ScarUtilAffinitySet {
                node_id: idx,
                packed,
            },
        };
        self.wal.append(rec)?;

        self.with_node_zone(|zf| {
            let offset = idx as usize * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, self.min_headroom)?;
            let cur: NodeRecord = {
                let bytes = &zf.as_slice()[offset..offset + NodeRecord::SIZE];
                *bytemuck::from_bytes::<NodeRecord>(bytes)
            };
            let mut updated = cur;
            updated.scar_util_affinity = packed;
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(())
        })
    }

    /// Update the 5-bit scar sub-field of a node's packed `scar_util_affinity`
    /// column under the zone lock. Utility and affinity bits are preserved;
    /// the dirty flag is forced to 1. Emits an absolute
    /// [`WalPayload::ScarUtilAffinitySet`] (idempotent replay).
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn update_scar_field(&self, idx: u32, scar_q5: u8) -> SubstrateResult<()> {
        self.update_scar_util_aff_field(idx, |p| {
            crate::record::PackedScarUtilAff::new(scar_q5, p.utility, p.affinity, true)
        })
    }

    /// Update the 5-bit utility sub-field of the packed column.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn update_utility_field(&self, idx: u32, util_q5: u8) -> SubstrateResult<()> {
        self.update_scar_util_aff_field(idx, |p| {
            crate::record::PackedScarUtilAff::new(p.scar, util_q5, p.affinity, true)
        })
    }

    /// Update the 5-bit affinity sub-field of the packed column.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn update_affinity_field(&self, idx: u32, aff_q5: u8) -> SubstrateResult<()> {
        self.update_scar_util_aff_field(idx, |p| {
            crate::record::PackedScarUtilAff::new(p.scar, p.utility, aff_q5, true)
        })
    }

    /// Internal read-modify-write primitive for the `scar_util_affinity` u16
    /// column. The whole read + transform + write + WAL append happens under
    /// the zone lock so concurrent sub-field updates from different stores
    /// cannot clobber each other.
    fn update_scar_util_aff_field<F>(&self, idx: u32, transform: F) -> SubstrateResult<()>
    where
        F: FnOnce(crate::record::PackedScarUtilAff) -> crate::record::PackedScarUtilAff,
    {
        self.with_node_zone(|zf| {
            let offset = idx as usize * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, self.min_headroom)?;
            let cur: NodeRecord = {
                let bytes = &zf.as_slice()[offset..offset + NodeRecord::SIZE];
                *bytemuck::from_bytes::<NodeRecord>(bytes)
            };
            let current_packed = crate::record::PackedScarUtilAff::unpack(cur.scar_util_affinity);
            let new_packed = transform(current_packed).pack();

            let rec = WalRecord {
                lsn: 0,
                timestamp: unix_micros(),
                flags: 0,
                payload: WalPayload::ScarUtilAffinitySet {
                    node_id: idx,
                    packed: new_packed,
                },
            };
            self.wal.append(rec)?;

            let mut updated = cur;
            updated.scar_util_affinity = new_packed;
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(())
        })
    }

    /// Apply an eager decay to every live edge's synapse weight column:
    /// `weight ← weight × factor / 65536` (Q0.16 multiplier).
    ///
    /// Sibling to [`Self::decay_all_energy`] but operating on [`EdgeRecord`]
    /// rather than [`NodeRecord`]. The Consolidator Thinker (T13) schedules
    /// both in the same batch tick, so a single transaction covers node-
    /// and edge-side cognitive decay.
    ///
    /// WAL payload: [`WalPayload::SynapseDecay`] (single compact record).
    ///
    /// `edge_high_water` is the exclusive upper bound of live edge slots
    /// (`SubstrateStore::edge_slot_high_water()`). Tombstoned slots are
    /// skipped. Slot 0 is reserved and never touched.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn decay_all_synapse(&self, factor_q16: u16, edge_high_water: u64) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::SynapseDecay { factor_q16 },
        };
        self.wal.append(rec)?;

        self.with_edge_zone(|zf| {
            apply_synapse_decay_to_zone(zf, factor_q16, edge_high_water);
            Ok(())
        })
    }

    // ==================================================================
    // Coactivation column primitives (T7 Step 5)
    // ==================================================================
    //
    // COACT edges share `EdgeRecord.weight_u16` with synapse edges but
    // are distinguished by `edge_type == coact_type_id` (interned via
    // [`crate::record::COACT_EDGE_TYPE_NAME`]). The decay schedule is
    // independent — COACT decays slowly, synapse decays fast — so we
    // ship a dedicated WAL payload [`WalPayload::CoactDecay`] that
    // carries the type id alongside the multiplicative factor.
    //
    // Reinforcement uses [`WalPayload::SynapseReinforce`] (which carries
    // the absolute post-state and is therefore agnostic to edge type) so
    // we don't grow the payload set unnecessarily — the coact_type_id
    // is implicit in the edge slot itself.

    /// Saturating-add a Q0.16 delta to the `weight_u16` column of edge
    /// slot `idx`, then log the **absolute** post-state via
    /// [`WalPayload::SynapseReinforce`] (so replay is idempotent).
    ///
    /// Designed for COACT edges (each coactivation adds a small δ); also
    /// usable for any other Q0.16 weight column whose semantics are
    /// "accumulate evidence with saturation at 1.0".
    ///
    /// Returns the new weight in Q0.16 so the caller can monitor
    /// saturation without a follow-up read.
    ///
    /// ⚠ Caller must verify that `idx` resolves to a live COACT edge —
    /// this primitive does **not** check `edge_type` (it's a column
    /// operation, not a typed-edge operation). The store-level
    /// `crate::store::SubstrateStore::coact_reinforce` enforces typing.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn coact_reinforce_at(&self, idx: u64, delta_q16: u16) -> SubstrateResult<u16> {
        self.with_edge_zone(|zf| {
            let offset = idx as usize * EdgeRecord::SIZE;
            ensure_room(zf, offset + EdgeRecord::SIZE, self.min_headroom)?;
            let cur: EdgeRecord = {
                let bytes = &zf.as_slice()[offset..offset + EdgeRecord::SIZE];
                *bytemuck::from_bytes::<EdgeRecord>(bytes)
            };
            let new_weight = cur.weight_u16.saturating_add(delta_q16);

            // (1) WAL first — log absolute post-state for idempotent replay.
            let rec = WalRecord {
                lsn: 0,
                timestamp: unix_micros(),
                flags: 0,
                payload: WalPayload::SynapseReinforce {
                    edge_id: idx,
                    new_weight,
                },
            };
            self.wal.append(rec)?;

            // (2) mmap mutation under the same zone lock — no reader can
            //     observe an interleaved column / WAL state.
            let mut updated = cur;
            updated.weight_u16 = new_weight;
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(new_weight)
        })
    }

    /// Apply a Q0.16 multiplicative decay to every live edge whose
    /// `edge_type` equals `coact_type_id`. Synapse edges and other types
    /// are untouched.
    ///
    /// WAL payload: [`WalPayload::CoactDecay { factor_q16, coact_type_id }`].
    ///
    /// `edge_high_water` is the exclusive upper bound of allocated edge
    /// slots (`SubstrateStore::edge_slot_high_water()`). Slot 0 is
    /// reserved.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn decay_all_coact(
        &self,
        factor_q16: u16,
        edge_high_water: u64,
        coact_type_id: u16,
    ) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::CoactDecay {
                factor_q16,
                coact_type_id,
            },
        };
        self.wal.append(rec)?;

        self.with_edge_zone(|zf| {
            apply_coact_decay_to_zone(zf, factor_q16, edge_high_water, coact_type_id);
            Ok(())
        })
    }

    /// Reinforce a synapse (edge weight column) to `new_weight_u16` (Q0.16).
    ///
    /// WAL payload: [`WalPayload::SynapseReinforce`].
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn reinforce_synapse(&self, idx: u64, new_weight_u16: u16) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::SynapseReinforce {
                edge_id: idx,
                new_weight: new_weight_u16,
            },
        };
        self.wal.append(rec)?;

        self.with_edge_zone(|zf| {
            let offset = idx as usize * EdgeRecord::SIZE;
            ensure_room(zf, offset + EdgeRecord::SIZE, self.min_headroom)?;
            let cur: EdgeRecord = {
                let bytes = &zf.as_slice()[offset..offset + EdgeRecord::SIZE];
                *bytemuck::from_bytes::<EdgeRecord>(bytes)
            };
            let mut updated = cur;
            updated.weight_u16 = new_weight_u16;
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(())
        })
    }

    /// Batch-update centrality for a list of nodes.
    ///
    /// WAL payload: [`WalPayload::CentralityUpdate`].
    #[tracing::instrument(level = "trace", skip(self, updates))]
    pub fn update_centrality_batch(&self, updates: Vec<(u32, u16)>) -> SubstrateResult<()> {
        self.note_feature_mutation();
        if updates.is_empty() {
            return Ok(());
        }
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::CentralityUpdate {
                updates: updates.clone(),
            },
        };
        self.wal.append(rec)?;

        self.with_node_zone(|zf| {
            for (idx, value) in &updates {
                let offset = (*idx as usize) * NodeRecord::SIZE;
                ensure_room(zf, offset + NodeRecord::SIZE, self.min_headroom)?;
                let cur: NodeRecord = {
                    let bytes = &zf.as_slice()[offset..offset + NodeRecord::SIZE];
                    *bytemuck::from_bytes::<NodeRecord>(bytes)
                };
                let mut updated = cur;
                updated.centrality_cached = *value;
                // Clear the CENTRALITY_STALE flag if set.
                updated.flags &= !crate::record::node_flags::CENTRALITY_STALE;
                zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                    .copy_from_slice(bytemuck::bytes_of(&updated));
            }
            Ok(())
        })
    }

    /// Batch-update community_id for a list of nodes.
    ///
    /// Used by LDleiden (T10) to persist node→community assignments after an
    /// incremental update. Just like centrality, the WAL payload carries the
    /// full `(node_id, community_id)` tuples so replay is idempotent.
    ///
    /// WAL payload: [`WalPayload::CommunityAssign`].
    #[tracing::instrument(level = "trace", skip(self, updates))]
    pub fn update_community_batch(&self, updates: Vec<(u32, u32)>) -> SubstrateResult<()> {
        self.note_feature_mutation();
        if updates.is_empty() {
            return Ok(());
        }
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::CommunityAssign {
                updates: updates.clone(),
            },
        };
        self.wal.append(rec)?;

        self.with_node_zone(|zf| {
            for (idx, community_id) in &updates {
                let offset = (*idx as usize) * NodeRecord::SIZE;
                ensure_room(zf, offset + NodeRecord::SIZE, self.min_headroom)?;
                let cur: NodeRecord = {
                    let bytes = &zf.as_slice()[offset..offset + NodeRecord::SIZE];
                    *bytemuck::from_bytes::<NodeRecord>(bytes)
                };
                let mut updated = cur;
                updated.community_id = *community_id;
                zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                    .copy_from_slice(bytemuck::bytes_of(&updated));
            }
            Ok(())
        })
    }

    // ==================================================================
    // Hilbert page ordering (T11 Step 2)
    // ==================================================================

    /// Bulk-sort node slots by `(community_id, hilbert_key(centrality, degree))`.
    ///
    /// This is the one-shot reorder invoked by the caller (typically at
    /// first-open after community detection has run at least once, or
    /// after a significant mutation burst). Semantics:
    ///
    /// 1. Scan `Zone::Nodes` to collect `(community_id, centrality_cached,
    ///    tombstoned)` per slot.
    /// 2. Scan `Zone::Edges` once to compute out-degree per source slot.
    /// 3. Compute the `old_to_new` permutation via
    ///    [`compute_hilbert_permutation`](crate::hilbert::compute_hilbert_permutation). Nodes with the same community
    ///    become contiguous; within a community they're ordered by a 2D
    ///    Hilbert curve on `(centrality, degree)`; tombstoned slots move
    ///    to the tail.
    /// 4. Emit a [`WalPayload::HilbertRepermute`] WAL record carrying the
    ///    permutation — WAL-first discipline.
    /// 5. Apply the permutation **out-of-place** to `Zone::Nodes` (build
    ///    a fresh buffer, swap at the end).
    /// 6. Rewrite every `EdgeRecord.src` / `dst` using the permutation.
    /// 7. Rebuild `Zone::Hilbert`, `Zone::Zorder` and `Zone::Community`
    ///    side-columns so each new slot carries its hilbert_key /
    ///    zorder_key / community_id — these columns are the fast-path for
    ///    spreading activation prefetch (`madvise(WILLNEED)` range
    ///    computation) and for Z-order window queries on the feature plane.
    /// 8. Set [`meta_flags::HILBERT_SORTED`] and persist the meta header.
    /// 9. `msync` + `fsync` touched zones + commit WAL.
    ///
    /// ### Parameters
    ///
    /// * `node_high_water` — exclusive upper bound on allocated node slots
    ///   (the caller usually tracks this via its graph manager).
    /// * `edge_high_water` — exclusive upper bound on allocated edge slots.
    /// * `order` — Hilbert resolution in bits per axis. Typical values:
    ///   6 (64×64 grid) for KBs up to 10⁵ nodes, 8 (256×256) for 10⁶+.
    ///   Persisted into [`crate::meta::MetaHeader::hilbert_order`].
    /// * `max_degree` — saturation cap for the degree axis.
    ///
    /// ### Crash safety (Step 2 scope)
    ///
    /// This Step 2 implementation is **not** transactionally crash-safe:
    /// a crash between (5) and (6) leaves the substrate in an intermediate
    /// state where the node zone is permuted but edges still reference the
    /// old slot IDs. Callers MUST restrict `bulk_sort_by_hilbert` to a
    /// quiet window (no concurrent writes, no expected mid-operation
    /// shutdown). The full transactional variant with out-of-place journal
    /// + atomic swap lands in T11 Step 4 (`CompactCommunity` pass).
    ///
    /// ### Idempotency
    ///
    /// Early-returns if `HILBERT_SORTED` is already set in the meta flags,
    /// so accidental re-invocation is safe. Callers that want to re-sort
    /// after major mutations must first clear the flag (a privileged op
    /// exposed via `SubstrateFile::write_meta_header`) — this explicit gate
    /// is deliberate to prevent pointless re-permutations.
    #[tracing::instrument(level = "info", skip(self))]
    pub fn bulk_sort_by_hilbert(
        &self,
        node_high_water: u32,
        edge_high_water: u64,
        order: u32,
        max_degree: u32,
    ) -> SubstrateResult<()> {
        // T4 simplicial : les zones simpliciales ne sont pas encore
        // enrôlées dans le remap de slots — refus explicite plutôt que
        // corruption silencieuse.
        self.refuser_remap_si_simplexes("bulk_sort_by_hilbert")?;
        // Early-out idempotency check.
        {
            let sub = self.substrate.lock();
            let h = sub.meta_header();
            if h.flags & meta_flags::HILBERT_SORTED != 0 {
                tracing::debug!("substrate already Hilbert-sorted, skipping");
                return Ok(());
            }
        }

        if node_high_water == 0 {
            return Ok(());
        }

        // ---- Phase 1 — read node columns + compute degrees -------------
        //
        // A NodeRecord is treated as `tombstoned` for sort purposes when:
        //   - its TOMBSTONED flag is set (explicit delete), OR
        //   - it is a zero-sentinel (all fields zero) left behind by
        //     slow-path alignment in
        //     `SubstrateStore::allocate_node_id_in_community` (T11 Step 3).
        //     Skipped-slot zero-fill from `ZoneFile::grow_to` has no
        //     community membership; classifying it as tombstoned pushes it
        //     to the tail post-sort instead of silently inflating the
        //     zero-community run.
        //
        // Slot 0 is always the null-sentinel and is likewise treated as
        // tombstoned so the sort never needs to spare it from the tail.
        let mut communities = vec![0u32; node_high_water as usize];
        let mut centrality = vec![0u16; node_high_water as usize];
        let mut tombstoned = vec![false; node_high_water as usize];
        self.with_node_zone(|zf| {
            for idx in 0..node_high_water {
                let offset = (idx as usize) * NodeRecord::SIZE;
                if offset + NodeRecord::SIZE > zf.as_slice().len() {
                    break;
                }
                let rec: &NodeRecord =
                    bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
                // (jalon 1) ALLOCATED remplace la zero-sentinelle 8-champs.
                communities[idx as usize] = rec.community_id;
                centrality[idx as usize] = rec.centrality_cached;
                tombstoned[idx as usize] = !rec.is_live() || idx == 0;
            }
            Ok(())
        })?;

        let mut degrees = vec![0u32; node_high_water as usize];
        self.with_edge_zone(|zf| {
            for idx in 0..edge_high_water {
                let offset = (idx as usize) * EdgeRecord::SIZE;
                if offset + EdgeRecord::SIZE > zf.as_slice().len() {
                    break;
                }
                let rec: &EdgeRecord =
                    bytemuck::from_bytes(&zf.as_slice()[offset..offset + EdgeRecord::SIZE]);
                if rec.is_tombstoned() {
                    continue;
                }
                if (rec.src as usize) < degrees.len() {
                    degrees[rec.src as usize] = degrees[rec.src as usize].saturating_add(1);
                }
            }
            Ok(())
        })?;

        // ---- Phase 2 — compute permutation ----------------------------
        //
        // Page-aligned: each community's run starts on a multiple of
        // `NODES_PER_PAGE` so a single community never straddles a page
        // boundary post-sort. Tomb/zero-sentinel slots (plentiful after
        // slow-path community alignment on insert) absorb the padding
        // holes between community runs.
        let old_to_new = compute_hilbert_permutation_page_aligned(
            &communities,
            &centrality,
            &degrees,
            &tombstoned,
            order,
            max_degree,
            NODES_PER_PAGE,
        );
        // (2026-09-06, fix CONSTAT-REMAP-PROPS-DETACHEMENT) Mémoïse la
        // permutation pour que `SubstrateStore` rafraîchisse les owner stamps
        // des chaînes PropsZone v2 des nœuds déplacés (post-sort). Clone O(n)
        // — le sort est déjà O(n log n), coût négligeable mesuré.
        self.derniere_permutation_remap
            .write()
            .replace(std::sync::Arc::new(old_to_new.clone()));

        // ---- Phase 3 — WAL-first: log the permutation -----------------
        //
        // Chantier SlotRemappable :
        // * I1 — la permutation est validée (bijection totale, 0 → 0)
        //   AVANT toute écriture ; un refus n'a rien muté.
        // * Le record V2 porte l'epoch de repermutation (monotone,
        //   `substrate.remap.epoch`).
        // * Les enregistrements ABSOLUS des zones engram (membres +
        //   bitsets post-remap) sont journalisés dans la MÊME fenêtre de
        //   durabilité que le record V2 (un seul fsync WAL), puis
        //   appliqués aux zones : le rejeu les ré-applique idempotemment
        //   quel que soit l'état intermédiaire — c'est leur mécanisme
        //   d'idempotence (les zones engram n'ont pas d'en-tête où
        //   porter un epoch).
        slot_remap::validate_permutation(&old_to_new)?;
        // La fenêtre de rejeu doit être VIDE avant d'écrire le record :
        // sinon un rejeu ultérieur repartirait d'avant le tri et
        // réécrirait des identifiants pré-permutation dans une zone
        // post-permutation. Voir `require_empty_replay_window`.
        self.require_empty_replay_window("bulk_sort_by_hilbert")?;

        let epoch = {
            let sub = self.substrate.lock();
            slot_remap::current_remap_epoch(&sub)?.saturating_add(1)
        };
        let (engram_member_updates, engram_bitset_updates) =
            self.compute_engram_remap_updates(&old_to_new)?;
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::HilbertRepermuteV2 {
                permutation: old_to_new.clone(),
                epoch,
                engram_members: engram_member_updates.clone(),
                engram_bitsets: engram_bitset_updates.clone(),
            },
        };
        self.wal.append(rec)?;
        // Point de durabilité : le record porte TOUT l'état absolu (y
        // compris engram) et est sur disque AVANT la première mutation.
        self.wal.fsync()?;

        // ANNONCE : à partir d'ici les zones peuvent être touchées sans
        // que leur durabilité soit acquise. Un rejeu qui retrouve cette
        // annonce sans la confirmation correspondante REFUSERA de
        // deviner — voir `slot_remap::RemapEpochState`.
        // La garde porte l'annonce ET sa resolution : elle ne peut pas
        // etre laissee non tranchee (voir `slot_remap::AnnonceRemap`).
        let annonce = slot_remap::AnnonceRemap::annoncer_partagee(self.substrate.clone(), epoch)?;

        // ---- Phases 4 a 7 — MUTATIONS DE ZONES, SOUS GARDE D'ANNONCE --
        //
        // Tout echec ici est traite par `abandonner_annonce_remap` : voir
        // sa doc pour le raisonnement complet. Resume : un echec AVANT la
        // premiere ecriture efface l'annonce (donnees intactes, base
        // ouvrable) ; un echec APRES la laisse (etat indetermine, la base
        // doit refuser de se rouvrir).
        let mutations = (|| -> SubstrateResult<()> {
            // ---- Phase 4 — apply permutation out-of-place to Zone::Nodes --
            self.with_node_zone(|zf| {
                let n = node_high_water as usize;
                let total = n * NodeRecord::SIZE;
                ensure_room(zf, total, self.min_headroom)?;
                let mut buf = vec![NodeRecord::default(); n];
                let src = &zf.as_slice()[..total];
                for old in 0..n {
                    let new = old_to_new[old] as usize;
                    let rec: &NodeRecord = bytemuck::from_bytes(
                        &src[old * NodeRecord::SIZE..(old + 1) * NodeRecord::SIZE],
                    );
                    buf[new] = *rec;
                }
                // ── PREMIERE MUTATION DE ZONE ──
                // Au-dela de cette ligne l'etat des zones n'est plus connu de
                // l'exterieur tant que la barriere (phase 7) n'est pas
                // franchie. `ensure_room` ci-dessus, lui, peut echouer sans
                // avoir rien touche : c'est LA fenetre que le drapeau protege.
                annonce.premiere_mutation();
                zf.as_slice_mut()[..total].copy_from_slice(bytemuck::cast_slice(&buf));
                Ok(())
            })?;

            // ---- Phase 5 — rewrite edge.src/dst in place ------------------
            self.with_edge_zone(|zf| {
                for idx in 0..edge_high_water {
                    let offset = (idx as usize) * EdgeRecord::SIZE;
                    if offset + EdgeRecord::SIZE > zf.as_slice().len() {
                        break;
                    }
                    let rec: EdgeRecord =
                        *bytemuck::from_bytes(&zf.as_slice()[offset..offset + EdgeRecord::SIZE]);
                    // Silent tombstones keep their (now stale) src/dst — their payload
                    // is ignored by every reader anyway.
                    if rec.is_tombstoned() {
                        continue;
                    }
                    let mut updated = rec;
                    if (rec.src as usize) < old_to_new.len() {
                        updated.src = old_to_new[rec.src as usize];
                    }
                    if (rec.dst as usize) < old_to_new.len() {
                        updated.dst = old_to_new[rec.dst as usize];
                    }
                    zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE]
                        .copy_from_slice(bytemuck::bytes_of(&updated));
                }
                Ok(())
            })?;

            // ---- Phase 6 — rebuild side-columns (Hilbert, Zorder, Community) --
            // Hilbert side-column: u32 per new slot (the recomputed hilbert_key).
            // Zorder side-column: u32 per new slot (the recomputed zorder_key —
            // same (cx, cy) grid as hilbert_key, see RFC zorder-superposition).
            // Community side-column: u32 per new slot (the community_id).
            {
                let sub = self.substrate.lock();
                let mut hilbert_zone = sub.open_zone(Zone::Hilbert)?;
                let need = (node_high_water as u64) * 4;
                ensure_room(&mut hilbert_zone, need as usize, self.min_headroom)?;
                let mut hbuf = vec![0u32; node_high_water as usize];
                let mut zbuf = vec![0u32; node_high_water as usize];
                let mut cbuf = vec![0u32; node_high_water as usize];
                for old in 0..node_high_water as usize {
                    let new = old_to_new[old] as usize;
                    hbuf[new] =
                        hilbert_key_from_features(centrality[old], degrees[old], order, max_degree);
                    zbuf[new] =
                        zorder_key_from_features(centrality[old], degrees[old], order, max_degree);
                    cbuf[new] = communities[old];
                }
                hilbert_zone.as_slice_mut()[..(need as usize)]
                    .copy_from_slice(bytemuck::cast_slice(&hbuf));
                hilbert_zone.msync()?;
                hilbert_zone.fsync()?;

                let mut zorder_zone = sub.open_zone(Zone::Zorder)?;
                ensure_room(&mut zorder_zone, need as usize, self.min_headroom)?;
                zorder_zone.as_slice_mut()[..(need as usize)]
                    .copy_from_slice(bytemuck::cast_slice(&zbuf));
                zorder_zone.msync()?;
                zorder_zone.fsync()?;

                let mut community_zone = sub.open_zone(Zone::Community)?;
                ensure_room(&mut community_zone, need as usize, self.min_headroom)?;
                community_zone.as_slice_mut()[..(need as usize)]
                    .copy_from_slice(bytemuck::cast_slice(&cbuf));
                community_zone.msync()?;
                community_zone.fsync()?;
            }

            // ---- Phase 6b — zones engram (états absolus du record) ---------
            self.apply_engram_remap_updates(&engram_member_updates, &engram_bitset_updates)?;

            // ---- Phase 7 — BARRIÈRE de durabilité des zones ---------------
            //
            // (2026-08-23, revue adversariale) Tout ce qui précède est mmap ;
            // rien n'est durable avant ce msync. C'est LA barrière : elle
            // sépare « les zones peuvent être dans n'importe quel état » de
            // « les zones sont permutées, et on peut le dire ».
            self.msync_zones()?;

            // ══ BARRIÈRE FRANCHIE — l'état bascule de INDÉTERMINÉ à
            //    DÉTERMINÉ-ET-PERMUTÉ. Tout échec au-delà de cette ligne
            //    appelle la reprise OPPOSÉE de celle d'avant. ══
            annonce.barriere_franchie();

            // ---- Phase 8 — CONFIRMATION de l'epoch ---------------------
            //
            // Écrite juste après la barrière, AVANT les side-cars : un
            // crash entre les deux laisse un `committed` exact (les zones
            // SONT permutées) et des side-cars en retard, que le rejeu
            // rattrape un par un grâce à leurs epochs d'en-tête.
            //
            // (2026-08-24, revue #3) ELLE EST DANS LA RÉGION GARDÉE ; et
            // depuis la revue #5, c'est la GARDE elle-même qui la porte :
            // `confirmer` gère son propre échec (il retente la
            // confirmation, les zones étant permutées et durables). La
            // confirmation ne peut donc plus être appelée « à côté » de la
            // région gardée — elle EST la résolution.
            Ok(())
        })();
        match mutations {
            Ok(()) => annonce.confirmer()?,
            Err(e) => {
                annonce.abandonner(&e);
                return Err(e);
            }
        }

        // ---- Phase 9 — side-cars, APRÈS la barrière (condition 2) ------
        //
        // Ordre par risque croissant : tier0 (table slot_to_offset seule),
        // puis le registre (degrés, blobcol, veccol — le plus délicat en
        // dernier, protégé par sa bascule atomique par rename).
        {
            // Portee COURTE : le mutex ne doit surtout pas etre tenu
            // pendant l'appel au registre (interblocage — voir la doc du
            // trait `SlotRemappable`).
            let sub = self.substrate.lock();
            slot_remap::remap_tier0_slot_table(&sub, &old_to_new, epoch)?;
        }
        self.remap_registry.remap_node_slots(
            &self.substrate,
            &old_to_new,
            epoch,
            RemapContext::Live,
        )?;

        // ---- Phase 10 — méta + commit ---------------------------------
        {
            let mut sub = self.substrate.lock();
            let mut h = sub.meta_header();
            h.flags |= meta_flags::HILBERT_SORTED;
            h.hilbert_order = order;
            // ÉCRITURE PARTAGÉE : `h` vient d'être relu sur disque (mmap de la
            // méta). `node_high_water` peut être périmé si un pair a agrandi le
            // store depuis notre ouverture : on ne réduit jamais `node_count`.
            h.node_count = h.node_count.max(node_high_water as u64);
            sub.write_meta_header(&h)?;
        }
        let _ = self.commit()?;
        // Grille de tri persistée (hint — jamais fatal, cf. write_zgrid).
        // Écrite APRÈS le commit : la colonne Zorder est durable (barrière
        // Ph7) et le fichier ne fait partie d'aucune discipline WAL.
        {
            let sub = self.substrate.lock();
            let _ = sub.write_zgrid(order, max_degree);
        }
        Ok(())
    }

    /// Chantier SlotRemappable — exige que la fenêtre de rejeu soit VIDE
    /// avant d'entamer une repermutation.
    ///
    /// ── LE DÉFAUT QUE CECI FERME (revue adversariale, 2026-08-23) ──
    ///
    /// Le rejeu de production ne repart PAS de zéro : il repart de
    /// `wal_offset_at_flush`, l'offset du dernier flush
    /// (`store.rs`, ouverture). Or une repermutation ne faisait pas
    /// avancer cet offset. Enchaînement observé en revue : trafic
    /// ordinaire, puis tri réussi (zones msyncées, side-cars estampillés),
    /// puis crash plus tard. Au redémarrage, le rejeu repart d'AVANT le
    /// tri et réécrit des enregistrements en numérotation PRÉ-permutation
    /// dans une zone déjà POST-permutation. Corruption silencieuse, CRC
    /// valides.
    ///
    /// Le remède est en amont : on n'entame une repermutation que si la
    /// fenêtre de rejeu est vide, c'est-à-dire si le dernier flush couvre
    /// tout le WAL écrit. L'appelant supporté
    /// ([`crate::store::SubstrateStore::repermuter_par_hilbert`]) encadre
    /// la passe de deux flushes ; ce garde-fou refuse tout autre usage
    /// plutôt que de le laisser corrompre.
    fn require_empty_replay_window(&self, operation: &str) -> SubstrateResult<()> {
        let baseline = self.wal_baseline.load(std::sync::atomic::Ordering::Acquire);
        let current = self.wal.offset();
        if baseline < current {
            return Err(SubstrateError::Internal(format!(
                "{operation}: la fenetre de rejeu n'est pas vide (baseline {baseline} < WAL \
                 {current}). Une repermutation doit etre encadree par un flush AVANT et un \
                 flush APRES — passez par SubstrateStore::repermuter_par_hilbert."
            )));
        }
        Ok(())
    }

    /// Publie l'offset WAL couvert par le dernier flush durable. Appelé
    /// par [`crate::store::SubstrateStore::flush`] ; c'est ce que
    /// [`Self::require_empty_replay_window`] vérifie.
    /// Un `store` et non un `fetch_max` : un checkpoint TRONQUE le WAL et
    /// remet son offset a zero. Garder le maximum historique laisserait
    /// croire, apres troncature, que la fenetre de rejeu est vide alors
    /// qu'elle se remplit a nouveau.
    ///
    /// ══ VISIBILITE RESTREINTE (revue #3) ══
    ///
    /// Cette fonction ETAIT `pub`. Un appelant pouvait donc AFFIRMER une
    /// durabilite qui n'existait pas, et la garde
    /// [`Self::require_empty_replay_window`] le croyait. Constate en
    /// revue par construction d'un cas : en l'appelant sans flush reel on
    /// obtient une base dont l'ouverture rejoue des `update_node` en
    /// numerotation PRE-permutation sur des zones POST-permutation —
    /// exactement la corruption que cette garde existe pour empecher.
    ///
    /// Elle est desormais `pub(crate)`, et son seul appelant de
    /// production est `SubstrateStore::flush`, qui l'appelle APRES avoir
    /// reellement rendu les donnees durables. Les appelants externes qui
    /// ont besoin d'une fenetre vide doivent passer par
    /// [`Self::vider_fenetre_de_rejeu`], qui ne l'AFFIRME pas : elle la
    /// REND vide.
    pub(crate) fn note_durable_baseline(&self, offset: u64) {
        self.wal_baseline
            .store(offset, std::sync::atomic::Ordering::Release);
    }

    /// REND la fenetre de rejeu vide, au lieu de l'affirmer — remplacant
    /// public de `note_durable_baseline` (revue #3).
    ///
    /// Trois actes, dans cet ordre, et c'est l'ordre qui fait la surete :
    ///
    /// 1. `msync_zones` — tout ce que le WAL decrit est desormais dans
    ///    les zones, sur disque ;
    /// 2. `fsync` du WAL — rien n'est perdu si l'etape 3 echoue a
    ///    mi-chemin ;
    /// 3. `truncate` du WAL — la fenetre de rejeu est VIDE, et elle l'est
    ///    DURABLEMENT : plus aucun enregistrement pre-permutation ne peut
    ///    etre rejoue sur des zones post-permutation.
    ///
    /// Aucune donnee n'est perdue : l'etape 1 precede l'etape 3, donc
    /// tout ce qui est tronque est deja dans les zones. C'est exactement
    /// ce que fait un checkpoint.
    ///
    /// Contrairement a l'ancienne API, il n'y a rien a mentir ici : la
    /// fonction ne prend aucun parametre et ne croit personne sur parole.
    pub fn vider_fenetre_de_rejeu(&self) -> SubstrateResult<()> {
        self.msync_zones()?;
        self.wal.fsync()?;
        self.wal.truncate()?;
        self.wal_baseline
            .store(self.wal.offset(), std::sync::atomic::Ordering::Release);
        Ok(())
    }

    // ==================================================================
    // Chantier SlotRemappable — remap des zones engram
    // ==================================================================

    /// Calcule (LECTURE SEULE, avant tout WAL/mutation) les états
    /// absolus post-remap des zones engram :
    ///
    /// * membres — pour chaque engram dont la liste change, la liste
    ///   complète avec chaque slot nœud passé par `old_to_new` ;
    /// * bitsets — pour chaque slot dont la signature change, la valeur
    ///   u64 finale (permutation du tableau `bitset[new] = bitset[old]`).
    ///
    /// Les listes rendues sont journalisées telles quelles
    /// (`EngramMembersSet` / `EngramBitsetSet`) puis appliquées par
    /// [`Self::apply_engram_remap_updates`].
    #[allow(clippy::type_complexity)]
    fn compute_engram_remap_updates(
        &self,
        old_to_new: &[u32],
    ) -> SubstrateResult<(Vec<(u16, Vec<u32>)>, Vec<(u32, u64)>)> {
        // ── Membres : remap de VALEURS (les payloads sont des slots) ──
        let mut member_updates: Vec<(u16, Vec<u32>)> = Vec::new();
        self.with_engram_zone(|zf| {
            // Zone jamais initialisée (aucun engram) ou en-tête invalide :
            // rien à remapper — on ne force pas l'init pour un no-op.
            if crate::engram::EngramZone::payload_used(zf).is_err() {
                return Ok(());
            }
            for id in 1..crate::engram::DIRECTORY_SLOTS {
                let engram_id = id as u16;
                let Some(members) = crate::engram::EngramZone::members(zf, engram_id)? else {
                    continue;
                };
                let mapped: Vec<u32> = members
                    .iter()
                    .map(|&m| old_to_new.get(m as usize).copied().unwrap_or(m))
                    .collect();
                if mapped != members {
                    member_updates.push((engram_id, mapped));
                }
            }
            Ok(())
        })?;

        // ── Bitsets : permutation du tableau u64 sur [0, hw) ──────────
        let mut bitset_updates: Vec<(u32, u64)> = Vec::new();
        self.with_engram_bitset_zone(|zf| {
            let hw = old_to_new.len();
            let covered = (zf.as_slice().len() / crate::engram_bitset::BITSET_ENTRY_SIZE).min(hw);
            let mut old_vals = vec![0u64; hw];
            for (slot, val) in old_vals.iter_mut().enumerate().take(covered) {
                *val = crate::engram_bitset::EngramBitsetColumn::get(zf, slot as u32);
            }
            let mut new_vals = vec![0u64; hw];
            for (old, &new) in old_to_new.iter().enumerate() {
                new_vals[new as usize] = old_vals[old];
            }
            for slot in 0..hw {
                if new_vals[slot] != old_vals[slot] {
                    bitset_updates.push((slot as u32, new_vals[slot]));
                }
            }
            Ok(())
        })?;

        Ok((member_updates, bitset_updates))
    }

    fn apply_engram_remap_updates(
        &self,
        member_updates: &[(u16, Vec<u32>)],
        bitset_updates: &[(u32, u64)],
    ) -> SubstrateResult<()> {
        if !member_updates.is_empty() {
            self.with_engram_zone(|zf| {
                for (engram_id, members) in member_updates {
                    crate::engram::EngramZone::set_members_raw(zf, *engram_id, members)?;
                }
                Ok(())
            })?;
        }
        if !bitset_updates.is_empty() {
            self.with_engram_bitset_zone(|zf| {
                for (node_id, bitset) in bitset_updates {
                    crate::engram_bitset::EngramBitsetColumn::set_raw(zf, *node_id, *bitset)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// T11 Step 5 — transactional per-community compaction.
    ///
    /// Relocates every live node of `community_id` from its current scattered
    /// slots to a fresh page-aligned contiguous range appended at the node
    /// zone's tail. Returns the list of `(old_slot, new_slot)` mappings and
    /// the new high-water mark the caller must publish back to the store's
    /// slot allocator.
    ///
    /// # Durability protocol
    ///
    /// ```text
    ///   1. scan Nodes zone for live nodes with community_id == target
    ///   2. sort by hilbert_key (intra-community locality)
    ///   3. pick new_range = [aligned(current_hw), aligned(current_hw)+count)
    ///   4. WAL-append  CompactCommunity { cid, relocations, page_range }
    ///   5. fsync WAL   (durable commit boundary)
    ///   6. copy each old → new, zero-fill old
    ///   7. remap every edge whose (src or dst) is in old_slot set
    ///   8. rebuild Hilbert + Community side-columns for the affected slots
    ///   9. msync touched zones + WAL commit marker
    /// ```
    ///
    /// A crash between steps 4 and 9 is recovered idempotently from the WAL:
    /// the replay engine re-runs step 6–8 using the logged mapping.
    ///
    /// # Idempotency
    ///
    /// * Node relocation — only moves `old → new` when the node at `old`
    ///   still carries `community_id == target`. If replay finds the node
    ///   already at `new`, the record at `old` will be a zero-sentinel (we
    ///   zeroed it on first apply), the condition fails, and the move is
    ///   skipped.
    /// * Edge remap — only rewrites `src` / `dst` values that still live in
    ///   the `old_slot` domain. After the first pass, those values have
    ///   moved to the new-slot range (outside the keyset), so the lookup
    ///   misses and the second application is a no-op.
    ///
    /// Returns a struct holding the transaction summary so the caller can
    /// update its in-memory state (store slot allocator, community
    /// placements map) to match the new layout.
    #[tracing::instrument(level = "info", skip(self))]
    pub fn compact_community(
        &self,
        community_id: u32,
        current_node_hw: u32,
        current_edge_hw: u64,
        order: u32,
        max_degree: u32,
    ) -> SubstrateResult<CompactionResult> {
        // T4 simplicial : même garde que `bulk_sort_by_hilbert`.
        self.refuser_remap_si_simplexes("compact_community")?;
        if current_node_hw == 0 {
            return Ok(CompactionResult::empty());
        }

        // ---- Phase 1 — gather live members of the target community ----
        // (old_slot, centrality_cached) + ensemble des slots membres (pour le
        // comptage de degré restreint du scan d'arêtes, Phase 1b).
        let mut raw_members: Vec<(u32, u16)> = Vec::new();
        let mut member_slots: std::collections::HashSet<u32> = std::collections::HashSet::new();
        self.with_node_zone(|zf| {
            for slot in 0..current_node_hw {
                let offset = (slot as usize) * NodeRecord::SIZE;
                if offset + NodeRecord::SIZE > zf.as_slice().len() {
                    break;
                }
                let rec: &NodeRecord =
                    bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
                // (jalon 1) ALLOCATED remplace la garde zero-sentinelle :
                // tombstones et creneaux jamais ecrits exclus d'un test.
                if !rec.is_live() {
                    continue;
                }
                if rec.community_id != community_id {
                    continue;
                }
                raw_members.push((slot, rec.centrality_cached));
                member_slots.insert(slot);
            }
            Ok(())
        })?;

        if raw_members.is_empty() {
            return Ok(CompactionResult::empty());
        }

        // ---- Phase 1b — degrés SORTANTS RÉELS (Step 7) -----------------
        //
        // Un scan UNIQUE des arêtes, restreint aux sources membres, compte
        // le degré sortant exact de chaque slot. Le bulk sort fait déjà ce
        // scan O(E) ; la compaction est un chemin rare. AVANT ce changement,
        // hkey/zkey étaient calculés avec `degree = 0` — colonnes Hilbert et
        // Zorder dégradées par conception après compactage (les fenêtres 2D
        // et le préfetch lisaient des clés fausses sur l'axe degré).
        let mut outdeg: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        self.with_edge_zone(|zf| {
            for idx in 0..current_edge_hw {
                let offset = (idx as usize) * EdgeRecord::SIZE;
                if offset + EdgeRecord::SIZE > zf.as_slice().len() {
                    break;
                }
                let rec: &EdgeRecord =
                    bytemuck::from_bytes(&zf.as_slice()[offset..offset + EdgeRecord::SIZE]);
                if rec.flags & crate::record::edge_flags::TOMBSTONED != 0 {
                    continue;
                }
                if member_slots.contains(&rec.src) {
                    *outdeg.entry(rec.src).or_insert(0) += 1;
                }
            }
            Ok(())
        })?;

        let mut members: Vec<(u32, u32, u32)> = Vec::with_capacity(raw_members.len()); // (old_slot, hilbert_key, zorder_key)
        for (slot, centrality) in raw_members {
            let deg = outdeg.get(&slot).copied().unwrap_or(0);
            members.push((
                slot,
                hilbert_key_from_features(centrality, deg, order, max_degree),
                zorder_key_from_features(centrality, deg, order, max_degree),
            ));
        }

        // ---- Phase 2 — sort by (hilbert_key, old_slot) for determinism --
        members.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

        // ---- Phase 3 — pick new page-aligned range at the tail --------
        let ppg = NODES_PER_PAGE;
        let new_start = current_node_hw.div_ceil(ppg) * ppg;
        let count = members.len() as u32;
        let new_end = new_start.checked_add(count).ok_or_else(|| {
            SubstrateError::Internal("compact_community: slot-id overflow".into())
        })?;
        let first_page = new_start / ppg;
        let last_page = new_end.div_ceil(ppg);

        let relocations: Vec<(u32, u32)> = members
            .iter()
            .enumerate()
            .map(|(i, (old, _, _))| (*old, new_start + i as u32))
            .collect();

        // ---- Phase 4/5 — WAL-first + durabilité immédiate -------------
        //
        // Chantier SlotRemappable : permutation totale équivalente
        // (identité + échanges old ↔ new) validée AVANT toute écriture ;
        // record V2 porteur de l'epoch ; états absolus engram journalisés
        // dans la même fenêtre de durabilité (voir bulk_sort_by_hilbert,
        // Phase 3, pour le rationale complet).
        let total_perm = slot_remap::permutation_from_relocations(&relocations, new_end)?;
        self.require_empty_replay_window("compact_community")?;
        let epoch = {
            let sub = self.substrate.lock();
            slot_remap::current_remap_epoch(&sub)?.saturating_add(1)
        };
        let (engram_member_updates, engram_bitset_updates) =
            self.compute_engram_remap_updates(&total_perm)?;
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::CompactCommunityV2 {
                community_id,
                relocations: relocations.clone(),
                page_range: (first_page, last_page),
                epoch,
                engram_members: engram_member_updates.clone(),
                engram_bitsets: engram_bitset_updates.clone(),
            },
        };
        self.wal.append(rec)?;
        self.wal.fsync()?;
        // La garde porte l'annonce ET sa resolution : elle ne peut pas
        // etre laissee non tranchee (voir `slot_remap::AnnonceRemap`).
        let annonce = slot_remap::AnnonceRemap::annoncer_partagee(self.substrate.clone(), epoch)?;

        // Old → New maps for fast lookup during edge remap.
        let old_slots: std::collections::HashSet<u32> =
            relocations.iter().map(|(old, _)| *old).collect();
        let old_to_new_map: std::collections::HashMap<u32, u32> =
            relocations.iter().copied().collect();

        // ---- Phases 6 a 9 — MUTATIONS DE ZONES, SOUS GARDE D'ANNONCE --
        // Meme discipline que `bulk_sort_by_hilbert` — voir
        // `abandonner_annonce_remap`.
        let mutations = (|| -> SubstrateResult<()> {
            // ---- Phase 6 — copy old → new + zero old ----------------------
            self.with_node_zone(|zf| {
                let needed = (new_end as usize) * NodeRecord::SIZE;
                ensure_room(zf, needed, self.min_headroom)?;
                // Grab a snapshot of the old records first (borrowing rules).
                let mut copies: Vec<(u32, NodeRecord)> = Vec::with_capacity(relocations.len());
                {
                    let src = zf.as_slice();
                    for (old, new) in &relocations {
                        let off = (*old as usize) * NodeRecord::SIZE;
                        let rec: &NodeRecord =
                            bytemuck::from_bytes(&src[off..off + NodeRecord::SIZE]);
                        copies.push((*new, *rec));
                    }
                }
                // ── PREMIERE MUTATION DE ZONE ── (cf. bulk_sort_by_hilbert)
                annonce.premiere_mutation();
                // Write each to its new slot.
                let zone_mut = zf.as_slice_mut();
                for (new, rec) in &copies {
                    let off = (*new as usize) * NodeRecord::SIZE;
                    zone_mut[off..off + NodeRecord::SIZE].copy_from_slice(bytemuck::bytes_of(rec));
                }
                // Zero each old slot (turn into zero-sentinel — safe because we
                // set community_id back to 0, so the warden & bulk_sort will skip
                // it).
                let zero = NodeRecord::default();
                for (old, _) in &relocations {
                    let off = (*old as usize) * NodeRecord::SIZE;
                    zone_mut[off..off + NodeRecord::SIZE]
                        .copy_from_slice(bytemuck::bytes_of(&zero));
                }
                Ok(())
            })?;

            // ---- Phase 7 — remap edges whose endpoint is in old_slots -----
            self.with_edge_zone(|zf| {
                for idx in 0..current_edge_hw {
                    let offset = (idx as usize) * EdgeRecord::SIZE;
                    if offset + EdgeRecord::SIZE > zf.as_slice().len() {
                        break;
                    }
                    let rec: EdgeRecord =
                        *bytemuck::from_bytes(&zf.as_slice()[offset..offset + EdgeRecord::SIZE]);
                    if rec.is_tombstoned() {
                        continue;
                    }
                    let src_in = old_slots.contains(&rec.src);
                    let dst_in = old_slots.contains(&rec.dst);
                    if !src_in && !dst_in {
                        continue;
                    }
                    let mut updated = rec;
                    if src_in && let Some(&ns) = old_to_new_map.get(&rec.src) {
                        updated.src = ns;
                    }
                    if dst_in && let Some(&nd) = old_to_new_map.get(&rec.dst) {
                        updated.dst = nd;
                    }
                    zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE]
                        .copy_from_slice(bytemuck::bytes_of(&updated));
                }
                Ok(())
            })?;

            // ---- Phase 8 — rebuild affected slots in side-columns ---------
            {
                let sub = self.substrate.lock();
                let mut hilbert_zone = sub.open_zone(Zone::Hilbert)?;
                let needed = (new_end as usize) * 4;
                ensure_room(&mut hilbert_zone, needed, self.min_headroom)?;
                let hbytes = hilbert_zone.as_slice_mut();
                for (i, (old, new)) in relocations.iter().enumerate() {
                    let _ = old; // unused here, only hkey matters — recompute from sorted list
                    let hkey = members[i].1;
                    let off = (*new as usize) * 4;
                    hbytes[off..off + 4].copy_from_slice(&hkey.to_le_bytes());
                }
                // Zero side-column entries at old slots.
                for (old, _) in &relocations {
                    let off = (*old as usize) * 4;
                    hbytes[off..off + 4].copy_from_slice(&0u32.to_le_bytes());
                }
                hilbert_zone.msync()?;
                hilbert_zone.fsync()?;

                // Z-order side-column: same slots, same degraded-degree input
                // as the Hilbert key (mirror semantics — the warden's bulk
                // sort recomputes both with the real degree).
                let mut zorder_zone = sub.open_zone(Zone::Zorder)?;
                ensure_room(&mut zorder_zone, needed, self.min_headroom)?;
                let zbytes = zorder_zone.as_slice_mut();
                for (i, (old, new)) in relocations.iter().enumerate() {
                    let _ = old; // only zkey matters — from the sorted member list
                    let zkey = members[i].2;
                    let off = (*new as usize) * 4;
                    zbytes[off..off + 4].copy_from_slice(&zkey.to_le_bytes());
                }
                for (old, _) in &relocations {
                    let off = (*old as usize) * 4;
                    zbytes[off..off + 4].copy_from_slice(&0u32.to_le_bytes());
                }
                zorder_zone.msync()?;
                zorder_zone.fsync()?;

                let mut community_zone = sub.open_zone(Zone::Community)?;
                ensure_room(&mut community_zone, needed, self.min_headroom)?;
                let cbytes = community_zone.as_slice_mut();
                for (_, new) in &relocations {
                    let off = (*new as usize) * 4;
                    cbytes[off..off + 4].copy_from_slice(&community_id.to_le_bytes());
                }
                for (old, _) in &relocations {
                    let off = (*old as usize) * 4;
                    cbytes[off..off + 4].copy_from_slice(&0u32.to_le_bytes());
                }
                community_zone.msync()?;
                community_zone.fsync()?;
            }

            // ---- Phase 8b — zones engram (états absolus du record) --------
            self.apply_engram_remap_updates(&engram_member_updates, &engram_bitset_updates)?;

            // ---- Phase 9 — BARRIÈRE de durabilité, puis CONFIRMATION ------
            // Même discipline que `bulk_sort_by_hilbert` (voir ses phases
            // 7 à 9) : barrière → confirmation → side-cars.
            self.msync_zones()?;

            // ══ BARRIÈRE FRANCHIE (cf. bulk_sort_by_hilbert) ══
            annonce.barriere_franchie();
            Ok(())
        })();
        match mutations {
            Ok(()) => annonce.confirmer()?,
            Err(e) => {
                annonce.abandonner(&e);
                return Err(e);
            }
        }
        {
            let sub = self.substrate.lock();
            slot_remap::remap_tier0_slot_table(&sub, &total_perm, epoch)?;
        }
        self.remap_registry.remap_node_slots(
            &self.substrate,
            &total_perm,
            epoch,
            RemapContext::Live,
        )?;

        // ---- Phase 10 — méta (clear HILBERT_SORTED, bump node_count) --
        {
            let mut sub = self.substrate.lock();
            let mut h = sub.meta_header();
            h.flags &= !meta_flags::HILBERT_SORTED;
            // ÉCRITURE PARTAGÉE : même principe que `bulk_sort_by_hilbert`.
            // `current_node_hw` peut être périmé ; on ne réduit jamais `node_count`.
            h.node_count = h.node_count.max(new_end as u64);
            sub.write_meta_header(&h)?;
        }
        let _ = self.commit()?;

        Ok(CompactionResult {
            community_id,
            relocations,
            new_node_hw: new_end,
            page_range: (first_page, last_page),
        })
    }

    // ==================================================================
    // Engram membership side-table (T7 Step 0)
    // ==================================================================

    /// Write the full membership list for `engram_id` through the WAL-first
    /// path: append a [`WalPayload::EngramMembersSet`] record, then update
    /// the mmap'd side-table.
    ///
    /// Passing an empty `members` slice clears the engram's directory entry
    /// (no payload bytes written). `engram_id = 0` is reserved and rejected.
    #[tracing::instrument(level = "trace", skip(self, members))]
    pub fn set_engram_members(&self, engram_id: u16, members: Vec<u32>) -> SubstrateResult<()> {
        // (1) WAL first — payload carries full state for idempotent replay.
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::EngramMembersSet {
                engram_id,
                members: members.clone(),
            },
        };
        self.wal.append(rec)?;

        // (2) Side-table mutation.
        self.with_engram_zone(|zf| {
            crate::engram::EngramZone::set_members_raw(zf, engram_id, &members)
        })
    }

    /// Read the current membership list for `engram_id`.
    pub fn engram_members(&self, engram_id: u16) -> SubstrateResult<Option<Vec<u32>>> {
        self.with_engram_zone(|zf| crate::engram::EngramZone::members(zf, engram_id))
    }

    // ==================================================================
    // Engram bitset column (T7 Step 1)
    // ==================================================================

    /// Overwrite the 64-bit engram signature for `node_id` through the
    /// WAL-first path. Absolute semantics — the caller is responsible for
    /// folding in every engram the node still belongs to.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn set_engram_bitset(&self, node_id: u32, bitset: u64) -> SubstrateResult<()> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: 0,
            payload: WalPayload::EngramBitsetSet { node_id, bitset },
        };
        self.wal.append(rec)?;
        self.with_engram_bitset_zone(|zf| {
            crate::engram_bitset::EngramBitsetColumn::set_raw(zf, node_id, bitset)
        })
    }

    /// Convenience RMW: OR the mask for `engram_id` into `node_id`'s bitset.
    /// Reads the current bitset under the zone lock, ORs the new bit, logs
    /// the absolute post-state, then writes back.
    ///
    /// Does NOT remove stale bits — use [`Self::set_engram_bitset`] with a
    /// freshly-recomputed value for leave/remove semantics.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn add_engram_bit(&self, node_id: u32, engram_id: u16) -> SubstrateResult<()> {
        crate::engram_bitset::check_nonzero_engram_id(engram_id)?;
        let mask = crate::engram_bitset::engram_bit_mask(engram_id);
        self.with_engram_bitset_zone(|zf| {
            let current = crate::engram_bitset::EngramBitsetColumn::get(zf, node_id);
            let new_bits = current | mask;
            let rec = WalRecord {
                lsn: 0,
                timestamp: unix_micros(),
                flags: 0,
                payload: WalPayload::EngramBitsetSet {
                    node_id,
                    bitset: new_bits,
                },
            };
            self.wal.append(rec)?;
            crate::engram_bitset::EngramBitsetColumn::set_raw(zf, node_id, new_bits)
        })
    }

    /// Read the 64-bit engram signature for `node_id`.
    pub fn engram_bitset(&self, node_id: u32) -> SubstrateResult<u64> {
        self.with_engram_bitset_zone(|zf| {
            Ok(crate::engram_bitset::EngramBitsetColumn::get(zf, node_id))
        })
    }

    // ==================================================================
    // Hopfield recall (T7 Step 3)
    // ==================================================================

    /// Compute the top-`k` nodes by bitset overlap with `query_nid`'s
    /// signature. The scan runs under the engram-bitset zone lock through
    /// [`crate::engram_recall::top_k_by_overlap`], which dispatches to a
    /// SIMD kernel on aarch64 and to hardware POPCNT on x86_64.
    ///
    /// Returns `Vec<(node_id, overlap_bits)>` sorted by descending overlap
    /// (ties broken by ascending node id). Nodes with zero overlap are
    /// skipped.
    ///
    /// An all-zero query bitset (the node belongs to no engram) returns
    /// the empty vector without touching the column.
    ///
    /// **First tier only.** Callers must resolve Bloom collisions via
    /// [`Self::engram_members`] before using the candidates as actual
    /// co-members.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn hopfield_recall(&self, query_nid: u32, k: usize) -> SubstrateResult<Vec<(u32, u32)>> {
        let query = self.engram_bitset(query_nid)?;
        if query == 0 || k == 0 {
            return Ok(Vec::new());
        }
        self.with_engram_bitset_zone(|zf| {
            let bytes = zf.as_slice();
            // Zone capacity is always a multiple of BITSET_ENTRY_SIZE (8 B);
            // the bitset column is pre-allocated, so trailing slots read
            // back as 0 and contribute overlap=0 (naturally skipped).
            let entry = crate::engram_bitset::BITSET_ENTRY_SIZE;
            let n = bytes.len() / entry;
            // SAFETY: the zone is byte-aligned on mmap; u64 requires 8-byte
            // alignment which every mmapped page provides. We still use
            // bytemuck's checked cast to make the alignment promise explicit.
            let column: &[u64] = bytemuck::cast_slice(&bytes[..n * entry]);
            Ok(crate::engram_recall::top_k_by_overlap(column, query, k))
        })
    }

    // ==================================================================
    // Stockage simplicial (T4, décision 2d461b89, spec 271180c3)
    // ==================================================================

    /// Upsert WAL-first d'un simplexe (I1-I3, I7).
    ///
    /// Canonicalise les membres (tri strict sauf ORDERED, doublons et
    /// bornes rejetés), résout la clé (kind, type_id, key_hash) dans
    /// l'index d'unicité — départage par comparaison des membres en
    /// collision de hash — puis :
    ///
    /// * clé inconnue → alloue `simplex_id = high_water`, journalise
    ///   [`WalPayload::SimplexSet`] (état absolu), matérialise le record
    ///   (+ snapshot heap pour les hyperarêtes) et poste l'id dans
    ///   l'incidence de chaque membre ;
    /// * clé connue, contenu identique → no-op, AUCUN record WAL ;
    /// * clé connue, contenu réécrit (flags, ordre ORDERED, props) →
    ///   journalise + réécrit sous le même id, incidence intacte.
    ///
    /// Retourne `(simplex_id, créé)`. La liveness des slots de nœuds
    /// n'est pas vérifiée ici (primitive basse — même contrat que
    /// [`Writer::write_node`]) : `SubstrateStore::upsert_simplex` la
    /// vérifie.
    #[tracing::instrument(level = "trace", skip(self, members, props))]
    pub fn upsert_simplex(
        &self,
        kind: u8,
        type_id: u16,
        flags: u8,
        members: &[u32],
        props: &[u8],
    ) -> SubstrateResult<(crate::simplex_zone::SimplexId, bool)> {
        use crate::simplex_zone as sz;
        sz::validate_flags(kind, flags)?;
        let (stored, sorted_key) = sz::canonicalize_members(kind, flags, members)?;
        if kind != crate::record::simplex_kind::HYPEREDGE {
            if type_id != 0 {
                return Err(SubstrateError::WalBadFrame(
                    "type_id réservé aux hyperarêtes (0 pour les kinds 2-4)".into(),
                ));
            }
            if !props.is_empty() {
                return Err(SubstrateError::WalBadFrame(
                    "propriétés réservées aux hyperarêtes".into(),
                ));
            }
        }
        let key_hash = sz::key_hash_members(&sorted_key);

        self.with_simplex_zones(|z| {
            // ORDRE DE VERROUILLAGE : zones.write() (déjà tenu) puis index.
            let mut guard = self.simplex_index.lock();
            if guard.is_none() {
                *guard = Some(sz::SimplexKeyIndex::build(z.dir)?);
            }
            let idx = guard.as_mut().unwrap();

            if let Some(id) =
                idx.find_verified(z.dir, z.heap, kind, type_id, key_hash, &sorted_key)?
            {
                // Clé connue : contenu identique ⇒ no-op sans WAL.
                let rec = sz::SimplexDirZone::read(z.dir, id)?.ok_or_else(|| {
                    SubstrateError::WalBadFrame(format!(
                        "index simplicial désynchronisé : id {id} absent du répertoire"
                    ))
                })?;
                let (cur_members, cur_props) = sz::read_members_props(&rec, z.heap)?;
                if rec.flags == flags && cur_members == stored && cur_props == props {
                    return Ok((id, false));
                }
                // Réécriture sous le même id : WAL d'abord.
                let rec_wal = WalRecord {
                    lsn: 0,
                    timestamp: unix_micros(),
                    flags: 0,
                    payload: WalPayload::SimplexSet {
                        simplex_id: id,
                        kind,
                        flags,
                        type_id,
                        members: stored.clone(),
                        props: props.to_vec(),
                    },
                };
                self.wal.append(rec_wal)?;
                sz::apply_set_raw(z.dir, z.heap, id, kind, flags, type_id, &stored, props)?;
                return Ok((id, false));
            }

            // Clé inconnue : matérialisation à l'id suivant.
            sz::SimplexDirZone::init(z.dir)?;
            let id = sz::SimplexDirZone::high_water(z.dir)?;
            let rec_wal = WalRecord {
                lsn: 0,
                timestamp: unix_micros(),
                flags: 0,
                payload: WalPayload::SimplexSet {
                    simplex_id: id,
                    kind,
                    flags,
                    type_id,
                    members: stored.clone(),
                    props: props.to_vec(),
                },
            };
            self.wal.append(rec_wal)?;
            let applied =
                sz::apply_set_raw(z.dir, z.heap, id, kind, flags, type_id, &stored, props)?;
            debug_assert_eq!(applied, sz::AppliedSimplex::Materialized);
            for &m in &stored {
                crate::simplex_incidence::SimplexIncidenceZone::append(
                    z.incidence,
                    z.postings,
                    m,
                    id,
                )?;
            }
            idx.insert(kind, type_id, key_hash, id);
            Ok((id, true))
        })
    }

    /// Tombstone WAL-first d'un simplexe. Retourne `false` si le slot est
    /// vide ou déjà tombstoné. Les postings d'incidence restent en place
    /// (filtrés à la lecture, réclamés par [`Writer::compact_simplexes`]).
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn tombstone_simplex(&self, id: crate::simplex_zone::SimplexId) -> SubstrateResult<bool> {
        use crate::simplex_zone as sz;
        self.with_simplex_zones(|z| {
            let Some(rec) = sz::SimplexDirZone::read(z.dir, id)? else {
                return Ok(false);
            };
            if rec.is_tombstoned() {
                return Ok(false);
            }
            let rec_wal = WalRecord {
                lsn: 0,
                timestamp: unix_micros(),
                flags: 0,
                payload: WalPayload::SimplexTombstone { simplex_id: id },
            };
            self.wal.append(rec_wal)?;
            sz::SimplexDirZone::tombstone_raw(z.dir, id)?;
            let mut guard = self.simplex_index.lock();
            if let Some(idx) = guard.as_mut() {
                idx.remove(rec.kind, rec.type_id, sz::record_key_hash(&rec), id);
            }
            Ok(true)
        })
    }

    /// Lit un simplexe VIVANT. `None` pour un slot vide ou tombstoné.
    pub fn get_simplex(
        &self,
        id: crate::simplex_zone::SimplexId,
    ) -> SubstrateResult<Option<crate::simplex_zone::SimplexView>> {
        use crate::simplex_zone as sz;
        self.with_simplex_zones(|z| {
            let Some(rec) = sz::SimplexDirZone::read(z.dir, id)? else {
                return Ok(None);
            };
            if rec.is_tombstoned() {
                return Ok(None);
            }
            let (members, props) = sz::read_members_props(&rec, z.heap)?;
            Ok(Some(crate::simplex_zone::SimplexView {
                id,
                kind: rec.kind,
                flags: rec.flags,
                type_id: rec.type_id,
                members,
                props,
            }))
        })
    }

    /// Ids des simplexes VIVANTS incidents au nœud `node`, en O(degré
    /// simplicial) : lecture des postings + filtre de vie sur le record —
    /// jamais de scan global (spec §2).
    pub fn simplices_of_node(
        &self,
        node: u32,
    ) -> SubstrateResult<Vec<crate::simplex_zone::SimplexId>> {
        use crate::simplex_incidence::SimplexIncidenceZone;
        use crate::simplex_zone as sz;
        self.with_simplex_zones(|z| {
            if z.dir.is_empty() {
                return Ok(Vec::new());
            }
            let ids = SimplexIncidenceZone::postings(z.incidence, z.postings, node)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                if sz::SimplexDirZone::read(z.dir, id)?.is_some_and(|r| !r.is_tombstoned()) {
                    out.push(id);
                }
            }
            Ok(out)
        })
    }

    /// Ids des simplexes VIVANTS d'un kind donné (balayage du répertoire,
    /// O(nombre total de simplexes) — chemin d'itération, pas de requête).
    pub fn iter_simplices_kind(
        &self,
        kind: u8,
    ) -> SubstrateResult<Vec<crate::simplex_zone::SimplexId>> {
        use crate::simplex_zone as sz;
        self.with_simplex_zones(|z| {
            if z.dir.is_empty() {
                return Ok(Vec::new());
            }
            let hw = sz::SimplexDirZone::high_water(z.dir)?;
            let mut out = Vec::new();
            for id in 0..hw {
                if sz::SimplexDirZone::read(z.dir, id)?
                    .is_some_and(|r| r.kind == kind && !r.is_tombstoned())
                {
                    out.push(id);
                }
            }
            Ok(out)
        })
    }

    /// Cascade I5 : tombstone WAL-first de TOUS les simplexes vivants
    /// incidents à `node` (appelée par la suppression de nœud du store).
    /// O(degré simplicial du nœud) via l'incidence. Retourne les ids
    /// tombstonés.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn tombstone_simplexes_of_node(
        &self,
        node: u32,
    ) -> SubstrateResult<Vec<crate::simplex_zone::SimplexId>> {
        use crate::simplex_incidence::SimplexIncidenceZone;
        use crate::simplex_zone as sz;
        self.with_simplex_zones(|z| {
            if z.dir.is_empty() {
                return Ok(Vec::new());
            }
            let ids = SimplexIncidenceZone::postings(z.incidence, z.postings, node)?;
            let mut tombstoned = Vec::new();
            for id in ids {
                let Some(rec) = sz::SimplexDirZone::read(z.dir, id)? else {
                    continue;
                };
                if rec.is_tombstoned() {
                    continue;
                }
                let rec_wal = WalRecord {
                    lsn: 0,
                    timestamp: unix_micros(),
                    flags: 0,
                    payload: WalPayload::SimplexTombstone { simplex_id: id },
                };
                self.wal.append(rec_wal)?;
                sz::SimplexDirZone::tombstone_raw(z.dir, id)?;
                let mut guard = self.simplex_index.lock();
                if let Some(idx) = guard.as_mut() {
                    idx.remove(rec.kind, rec.type_id, sz::record_key_hash(&rec), id);
                }
                tombstoned.push(id);
            }
            Ok(tombstoned)
        })
    }

    /// Compaction déterministe des zones simpliciales (I4) : heap réécrit
    /// dans l'ordre des ids (snapshots orphelins réclamés), postings
    /// réécrits sans les ids tombstonés. Maintenance explicite — jamais
    /// sur le chemin du commit.
    pub fn compact_simplexes(&self) -> SubstrateResult<()> {
        use crate::simplex_incidence::SimplexIncidenceZone;
        use crate::simplex_zone as sz;
        self.with_simplex_zones(|z| {
            if z.dir.is_empty() {
                return Ok(());
            }
            sz::compact_heap(z.dir, z.heap)?;
            let dir: &ZoneFile = z.dir;
            SimplexIncidenceZone::compact_postings(z.incidence, z.postings, |_node, id| {
                sz::SimplexDirZone::read(dir, id)
                    .ok()
                    .flatten()
                    .is_some_and(|r| !r.is_tombstoned())
            })
        })
    }

    /// Écrit la configuration des passes simpliciales (T5) — WAL-first
    /// ([`WalPayload::SimplexPassesConfig`], état absolu) puis les octets
    /// 16..26 de l'en-tête SPXD. No-op sans record WAL si la config
    /// stockée est déjà identique (gabarit `upsert_simplex`).
    pub fn set_simplex_passes_config(
        &self,
        cfg: &crate::simplex_passes::SimplexPassesConfig,
    ) -> SubstrateResult<()> {
        use crate::simplex_passes as sp;
        self.with_simplex_zones(|z| {
            crate::simplex_zone::SimplexDirZone::init(z.dir)?;
            if sp::read_config_raw(z.dir)?.as_ref() == Some(cfg) {
                return Ok(());
            }
            let (de, dp, dh) = match cfg.dispatch {
                Some(d) => (d.edge_type, d.group_prop_key, d.he_type),
                None => (
                    sp::DISPATCH_ABSENT,
                    sp::DISPATCH_ABSENT,
                    sp::DISPATCH_ABSENT,
                ),
            };
            let rec = WalRecord {
                lsn: 0,
                timestamp: unix_micros(),
                flags: 0,
                payload: WalPayload::SimplexPassesConfig {
                    flags: cfg.flags,
                    clique_edge_type: cfg.clique_edge_type,
                    clique_label_mask: cfg.clique_label_mask,
                    dispatch_edge_type: de,
                    dispatch_prop_key: dp,
                    dispatch_he_type: dh,
                },
            };
            self.wal.append(rec)?;
            sp::write_config_raw(z.dir, cfg);
            *self.simplex_passes_cache.lock() = Some(Some(*cfg));
            Ok(())
        })
    }

    /// Variante CACHÉE de [`Writer::simplex_passes_config`] pour le chemin
    /// du commit : la première lecture paie l'aller-retour zone, les
    /// suivantes un mutex feuille non contendu.
    pub fn simplex_passes_config_cached(
        &self,
    ) -> SubstrateResult<Option<crate::simplex_passes::SimplexPassesConfig>> {
        if let Some(v) = *self.simplex_passes_cache.lock() {
            return Ok(v);
        }
        let v = self.simplex_passes_config()?;
        *self.simplex_passes_cache.lock() = Some(v);
        Ok(v)
    }

    /// Configuration des passes simpliciales, ou `None` si jamais
    /// configurée (zone vierge ou octets de config à zéro).
    pub fn simplex_passes_config(
        &self,
    ) -> SubstrateResult<Option<crate::simplex_passes::SimplexPassesConfig>> {
        self.with_simplex_zones(|z| {
            if z.dir.is_empty() {
                return Ok(None);
            }
            // Valide magic/version/longueur avant de toucher aux octets.
            crate::simplex_zone::SimplexDirZone::high_water(z.dir)?;
            crate::simplex_passes::read_config_raw(z.dir)
        })
    }

    /// Refuse toute repermutation de slots quand des simplexes vivants
    /// existent : membres et incidence sont indexés par SLOT de nœud, une
    /// permutation les rendrait silencieusement faux (CRC valides, données
    /// fausses). L'enrôlement des zones simpliciales dans le
    /// `SlotRemapRegistry` est le chantier de la passe de dérivation
    /// (T5+) ; d'ici là, le refus explicite est le seul comportement sûr.
    fn refuser_remap_si_simplexes(&self, operation: &str) -> SubstrateResult<()> {
        use crate::simplex_zone as sz;
        self.with_simplex_zones(|z| {
            if z.dir.is_empty() {
                return Ok(());
            }
            let hw = sz::SimplexDirZone::high_water(z.dir)?;
            for id in 0..hw {
                if sz::SimplexDirZone::read(z.dir, id)?.is_some_and(|r| !r.is_tombstoned()) {
                    return Err(SubstrateError::WalBadFrame(format!(
                        "{operation} refusé : la base porte des simplexes vivants (zone \
                         SimplexDir), qui référencent les slots de nœuds et ne sont pas \
                         encore enrôlés dans le remap de slots. Tombstonez la couche \
                         simpliciale ou attendez son enrôlement (T5+)."
                    )));
                }
            }
            Ok(())
        })
    }

    /// Seal the transaction: append a commit-flagged no-op and fsync.
    ///
    /// Under [`SyncMode::EveryCommit`] the `FLAG_COMMIT` bit triggers fsync
    /// automatically; under [`SyncMode::Group`] we still append the marker but
    /// no fsync is issued here — the caller owns it, explicitly, through
    /// [`Writer::fsync_wal`] (there is no timer in `Group` mode).
    ///
    /// (2026-11-01, D-DUR) `Session::commit()` calls this through
    /// `GraphStoreMut::seal_commit` on every committed transaction; it is no
    /// longer reserved to `flush()`.
    ///
    /// ── CE QUE DEVIENT CET APPEL SOUS GROUP COMMIT ARMÉ (2026-10-06) ────────
    /// Le `fsync` n'est plus indexé sur le LOT mais sur la NEEDLE (voir
    /// [`crate::wal_io::SEUIL_RECORDS`] / `SEUIL_OCTETS` / `SEUIL_TEMPS`) : ce
    /// `commit()` rend donc la main dès que le record est EN TAMPON, et la
    /// durabilité est acquise au plus tard `SEUIL_TEMPS` plus tard (ou avant si
    /// un seuil est franchi). C'est LE levier mesuré : 644 rec/s → ~1,5 M rec/s.
    ///
    /// Un appelant qui veut la durabilité AVANT le retour doit appeler
    /// [`Writer::commit_durable`] — c'est la seule différence entre les deux, et
    /// elle est explicite plutôt que devinée.
    ///
    /// Groupe commit NON armé (le défaut) : le comportement est celui d'avant,
    /// bit pour bit.
    pub fn commit(&self) -> SubstrateResult<u64> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: WalRecord::FLAG_COMMIT,
            payload: WalPayload::NoOp,
        };
        // (pièce 2) `append_encoded(&rec)` au lieu de `append(rec)` : plus de
        // `Vec<WalRecord>` alloué par `append_batch` pour un lot d'UN record.
        let (off, _) = self.wal.append_encoded(&rec)?;
        Ok(off)
    }

    /// ── COMMIT DURABLE : la même chose, mais DURABLE AVANT DE RENDRE LA MAIN ─
    ///
    /// Le record porte [`WalRecord::FLAG_SYNC_ATTENDU`] : le groupe commit pose
    /// alors le tampon et le `fsync` IMMÉDIATEMENT (voir
    /// `WalWriter::push_group_encode`), quel que soit l'état de la needle. Sans
    /// groupe commit armé, ce bit ne change RIEN au comportement (il n'est même
    /// pas écrit sur disque : voir la doc du bit) — c'est ce qui permet d'écrire
    /// `commit_durable()` partout sans régression.
    pub fn commit_durable(&self) -> SubstrateResult<u64> {
        let rec = WalRecord {
            lsn: 0,
            timestamp: unix_micros(),
            flags: WalRecord::FLAG_COMMIT | WalRecord::FLAG_SYNC_ATTENDU,
            payload: WalPayload::NoOp,
        };
        let (off, _) = self.wal.append_encoded(&rec)?;
        Ok(off)
    }

    /// ── ARMER LE GROUP COMMIT À NEEDLE DE DURABILITÉ (2026-10-06) ───────────
    ///
    /// # Effet
    /// Les records d'un `commit()` cessent d'être `fsync`és un par un : le
    /// `fsync` est indexé sur la needle (4 096 records / 4 Mio / 2 ms) et non sur
    /// le lot. Fenêtre de perte : bornée par `SEUIL_TEMPS`.
    ///
    /// # Pourquoi ce n'est pas le défaut
    /// Parce que c'est un CHANGEMENT DE SÉMANTIQUE de durabilité, pas un réglage
    /// de performance : `commit()` ne garantit plus « durable au retour ». Un
    /// appelant qui veut les deux appelle `commit_durable()`. Tant que cette
    /// méthode n'est pas appelée, rien ne change.
    pub fn activer_group_commit_wal(&self) {
        self.wal.activer_group_commit();
    }

    /// Le group commit à needle de durabilité est-il armé sur ce store ?
    pub fn group_commit_actif(&self) -> bool {
        self.wal.group_commit_actif()
    }

    /// Télémétrie du group commit (des zéros sans `OBRAIN_GC_DIAG=1`) : voir
    /// `wal_io::DiagGroupe`. C'est l'instrument qui a localisé la régression
    /// ×0,42-0,60 du 2026-10-06 ; le moteur la rend au banc.
    pub fn diagnostic_group_commit(&self) -> String {
        self.wal.diagnostic_group_commit()
    }

    /// Force la pose du tampon du group commit maintenant (et son `fsync`).
    /// Sans group commit armé : no-op.
    pub fn commit_point_wal(&self) -> SubstrateResult<()> {
        self.wal.commit_point()
    }

    /// Explicit WAL fsync — cheap durability checkpoint for [`SyncMode::Group`]
    /// (T5 batch-WAL, plan b30650e2): under `Group` mode `commit()` appends
    /// the marker but does NOT sync (the writer only fsyncs on
    /// `SyncMode::EveryCommit`). Callers batching many commits (imports,
    /// wardens) call this explicitly every N commits / at end-of-batch to
    /// bound how much work a crash can lose, without paying a syscall per
    /// commit. Cheap no-op-equivalent under `EveryCommit` (already synced
    /// per commit) and `Never` (intentionally not durable).
    pub fn fsync_wal(&self) -> SubstrateResult<()> {
        self.wal.fsync()
    }

    /// Msync all live zone mmaps. Call at end-of-checkpoint.
    /// Exclut les ecrivains le temps d'une operation critique.
    ///
    /// (2026-08-14) `checkpoint()` documentait « the caller MUST ensure no
    /// concurrent writes are in flight » — une exigence ECRITE mais JAMAIS
    /// APPLIQUEE. Tenir `substrate.lock()` ne suffisait pas : la voie
    /// d'ecriture prend `zones.write()`, et ne touche le mutex du substrat que
    /// pour l'ouverture PARESSEUSE d'une zone, une fois par zone et par
    /// instance.
    ///
    /// Cette fermeture rend l'exigence effective. Elle ne doit encadrer QUE le
    /// checkpoint proprement dit, jamais un `flush()` : `msync_zones` prend
    /// `zones.read()`, et `parking_lot::RwLock` n'est pas reentrant.
    ///
    /// Portee CLOSE plutot que garde rendue a l'appelant : le type interne
    /// `ZoneCache` ne fuit pas, et personne ne peut retenir l'exclusion
    /// au-dela de ce qu'il a ecrit.
    pub fn sous_exclusion_ecritures<R>(&self, f: impl FnOnce() -> R) -> R {
        let _garde = self.zones.write();
        f()
    }

    /// Msync toutes les zones vivantes, et ne `fsync` QUE celles dont la TAILLE
    /// a changé depuis le dernier `fsync` (extension/rétrécissement via
    /// `grow_to` / `shrink_to` / `rafraichir` — signalé par `remap_count`).
    ///
    /// ## Pourquoi ne pas fsync systématiquement (2026-10-06)
    ///
    /// `ZoneFile::msync` est un `msync(MS_SYNC)` : il pousse les pages de
    /// données sales sur le disque. Le `fsync` supplémentaire ne sert qu'à
    /// rendre durable la TAILLE du fichier (métadonnée d'inode) quand elle a
    /// bougé. L'ancien code enchaînait les deux sur CHAQUE zone à CHAQUE
    /// flush — un double flush redondant, payé ~8 fois par flush périodique.
    /// On ne garde donc `fsync` que pour les zones réellement étendues.
    pub fn msync_zones(&self) -> SubstrateResult<()> {
        let zones = self.zones.read();
        // Helper local : `msync` inconditionnel, `fsync` seulement si la
        // taille a changé (le compteur `remap_count` de la zone a bougé depuis
        // le dernier `fsync` mémorisé dans le champ `last_rc`).
        fn msync_zone_si_etendu(
            z: &ZoneFile,
            last_rc: &std::sync::atomic::AtomicU64,
        ) -> SubstrateResult<()> {
            z.msync()?;
            let rc = z.remap_count();
            if rc != last_rc.load(std::sync::atomic::Ordering::Acquire) {
                z.fsync()?;
                last_rc.store(rc, std::sync::atomic::Ordering::Release);
            }
            Ok(())
        }
        if let Some(z) = zones.nodes.as_ref() {
            msync_zone_si_etendu(z, &zones.nodes_fsync_rc)?;
        }
        if let Some(z) = zones.edges.as_ref() {
            msync_zone_si_etendu(z, &zones.edges_fsync_rc)?;
        }
        if let Some(z) = zones.engram_members.as_ref() {
            msync_zone_si_etendu(z, &zones.engram_members_fsync_rc)?;
        }
        if let Some(z) = zones.engram_bitset.as_ref() {
            msync_zone_si_etendu(z, &zones.engram_bitset_fsync_rc)?;
        }
        for (z, last_rc) in [
            (zones.simplex_dir.as_ref(), &zones.simplex_dir_fsync_rc),
            (zones.simplex_heap.as_ref(), &zones.simplex_heap_fsync_rc),
            (
                zones.simplex_incidence.as_ref(),
                &zones.simplex_incidence_fsync_rc,
            ),
            (
                zones.simplex_postings.as_ref(),
                &zones.simplex_postings_fsync_rc,
            ),
        ]
        .into_iter()
        .filter_map(|(z, rc)| z.map(|z| (z, rc)))
        {
            msync_zone_si_etendu(z, last_rc)?;
        }
        Ok(())
    }

    /// Issue `madvise(WILLNEED)` on a byte range of a zone (T11 Step 5
    /// prefetch hook). Best-effort — safe to call on cold zones (returns
    /// `Ok(())` silently).
    ///
    /// Zones covered:
    /// * [`Zone::Nodes`] / [`Zone::Edges`] / [`Zone::EngramMembers`] /
    ///   [`Zone::EngramBitset`] — go through the writer's zone cache.
    /// * [`Zone::Hilbert`] / [`Zone::Community`] / [`Zone::Props`] /
    ///   [`Zone::Strings`] / [`Zone::Tier0`] / [`Zone::Tier1`] /
    ///   [`Zone::Tier2`] — opened on-demand through the SubstrateFile.
    ///   A fresh `ZoneFile` is created per call and dropped; `madvise`
    ///   hits the kernel on the underlying file descriptor, so no
    ///   durability ordering is perturbed.
    pub fn advise_zone_willneed(
        &self,
        zone: Zone,
        offset: usize,
        len: usize,
    ) -> SubstrateResult<()> {
        match zone {
            Zone::Nodes => self.with_node_zone(|zf| zf.advise_willneed(offset, len)),
            Zone::Edges => self.with_edge_zone(|zf| zf.advise_willneed(offset, len)),
            Zone::EngramMembers => self.with_engram_zone(|zf| zf.advise_willneed(offset, len)),
            Zone::EngramBitset => {
                self.with_engram_bitset_zone(|zf| zf.advise_willneed(offset, len))
            }
            // One-shot zones: open a fresh handle, advise, drop. These
            // zones are not writer-managed in the ZoneCache because
            // mutations happen through dedicated paths (bulk_sort,
            // compact_community, etc.), not the normal hot-write loop.
            Zone::Hilbert
            | Zone::Community
            | Zone::Zorder
            | Zone::Props
            | Zone::Strings
            | Zone::Tier0
            | Zone::Tier1
            | Zone::Tier2
            | Zone::SimplexDir
            | Zone::SimplexIncidence => {
                let sub = self.substrate.lock();
                let zf = sub.open_zone(zone)?;
                drop(sub);
                zf.advise_willneed(offset, len)
            }
        }
    }

    // ----- internal helpers ------------------------------------------------

    fn with_node_zone<R>(
        &self,
        f: impl FnOnce(&mut ZoneFile) -> SubstrateResult<R>,
    ) -> SubstrateResult<R> {
        let mut zones = self.zones.write();
        if zones.nodes.is_none() {
            let sub = self.substrate.lock();
            zones.nodes = Some(sub.open_zone(Zone::Nodes)?);
        }
        f(zones.nodes.as_mut().unwrap())
    }

    /// Variante LECTURE de [`Writer::with_node_zone`] : plusieurs lecteurs
    /// courent ensemble.
    ///
    /// (2026-08-07) `read_node` passait par la variante exclusive et
    /// sérialisait donc toutes les lectures de nœuds de la base. La closure
    /// ne reçoit qu'un `&ZoneFile` — `as_slice(&self)` suffit à lire —, ce
    /// qui rend l'exclusivité inutile. Le `RwLock` continue d'exclure
    /// lecteurs et écrivain l'un de l'autre : la sûreté est inchangée, seule
    /// la concurrence entre lecteurs est rendue possible.
    ///
    /// Verrouillage à double vérification : on tente d'abord en lecture,
    /// et on ne bascule en écriture que pour l'ouverture paresseuse de la
    /// zone, qui n'arrive qu'une fois par zone et par instance.
    ///
    /// ATTENTION : `parking_lot::RwLock` n'est PAS réentrant. La closure ne
    /// doit jamais reprendre `self.zones`, sous peine d'interblocage.
    fn with_node_zone_read<R>(
        &self,
        f: impl FnOnce(&ZoneFile) -> SubstrateResult<R>,
    ) -> SubstrateResult<R> {
        {
            let zones = self.zones.read();
            if let Some(z) = zones.nodes.as_ref() {
                return f(z);
            }
        }
        let mut zones = self.zones.write();
        if zones.nodes.is_none() {
            let sub = self.substrate.lock();
            zones.nodes = Some(sub.open_zone(Zone::Nodes)?);
        }
        f(zones.nodes.as_ref().unwrap())
    }

    fn with_edge_zone<R>(
        &self,
        f: impl FnOnce(&mut ZoneFile) -> SubstrateResult<R>,
    ) -> SubstrateResult<R> {
        let mut zones = self.zones.write();
        if zones.edges.is_none() {
            let sub = self.substrate.lock();
            zones.edges = Some(sub.open_zone(Zone::Edges)?);
        }
        f(zones.edges.as_mut().unwrap())
    }

    /// Variante LECTURE de [`Writer::with_edge_zone`] — voir
    /// [`Writer::with_node_zone_read`] pour le raisonnement complet.
    fn with_edge_zone_read<R>(
        &self,
        f: impl FnOnce(&ZoneFile) -> SubstrateResult<R>,
    ) -> SubstrateResult<R> {
        {
            let zones = self.zones.read();
            if let Some(z) = zones.edges.as_ref() {
                return f(z);
            }
        }
        let mut zones = self.zones.write();
        if zones.edges.is_none() {
            let sub = self.substrate.lock();
            zones.edges = Some(sub.open_zone(Zone::Edges)?);
        }
        f(zones.edges.as_ref().unwrap())
    }

    fn with_engram_zone<R>(
        &self,
        f: impl FnOnce(&mut ZoneFile) -> SubstrateResult<R>,
    ) -> SubstrateResult<R> {
        let mut zones = self.zones.write();
        if zones.engram_members.is_none() {
            let sub = self.substrate.lock();
            zones.engram_members = Some(sub.open_zone(Zone::EngramMembers)?);
        }
        f(zones.engram_members.as_mut().unwrap())
    }

    fn with_engram_bitset_zone<R>(
        &self,
        f: impl FnOnce(&mut ZoneFile) -> SubstrateResult<R>,
    ) -> SubstrateResult<R> {
        let mut zones = self.zones.write();
        if zones.engram_bitset.is_none() {
            let sub = self.substrate.lock();
            zones.engram_bitset = Some(sub.open_zone(Zone::EngramBitset)?);
        }
        f(zones.engram_bitset.as_mut().unwrap())
    }

    /// Emprunte les QUATRE zones simpliciales sous le verrou d'écriture —
    /// une seule section critique pour (index → WAL → mmap), sans quoi deux
    /// upserts concurrents de la même clé matérialiseraient deux ids (I2).
    fn with_simplex_zones<R>(
        &self,
        f: impl FnOnce(&mut SimplexZones<'_>) -> SubstrateResult<R>,
    ) -> SubstrateResult<R> {
        let mut zones = self.zones.write();
        if zones.simplex_dir.is_none() {
            let sub = self.substrate.lock();
            zones.simplex_dir = Some(sub.open_zone(Zone::SimplexDir)?);
            zones.simplex_heap =
                Some(sub.open_named_zone(crate::simplex_zone::SIMPLEX_HEAP_FILENAME)?);
            zones.simplex_incidence = Some(sub.open_zone(Zone::SimplexIncidence)?);
            zones.simplex_postings =
                Some(sub.open_named_zone(crate::simplex_incidence::SIMPLEX_POSTINGS_FILENAME)?);
        }
        // Déstructuration : quatre emprunts disjoints du même cache.
        let ZoneCache {
            simplex_dir,
            simplex_heap,
            simplex_incidence,
            simplex_postings,
            ..
        } = &mut *zones;
        f(&mut SimplexZones {
            dir: simplex_dir.as_mut().unwrap(),
            heap: simplex_heap.as_mut().unwrap(),
            incidence: simplex_incidence.as_mut().unwrap(),
            postings: simplex_postings.as_mut().unwrap(),
        })
    }
}

/// Apply an energy-decay step in place to every live node slot in the zone.
///
/// `factor_q16` is a Q0.16 multiplier: `factor_q16 = 65535` is ≈ ×1.0,
/// `factor_q16 = 32768` halves. The operation is:
///
/// ```text
///   energy ← (energy × factor_q16) / 65536
/// ```
///
/// Tombstoned slots are skipped. `high_water` is the exclusive upper bound
/// of allocated slots (slot 0 is reserved). The function is side-effect
/// only — no WAL logging happens here; callers (`Writer::decay_all_energy`,
/// `replay::apply`) own the WAL logic.
///
/// The hot path processes 8 live slots at a time through
/// [`crate::simd::decay_u16x8`] (SSE2 on x86_64, NEON on aarch64, scalar
/// otherwise). The batch gathers 8 strided `energy` u16s into a stack
/// buffer, applies the SIMD mul-high, then scatters the results back —
/// skipping tombstoned slots via a per-slot flag test so the column keeps
/// its exact scalar semantics (bit-for-bit equivalent, property-tested in
/// [`crate::simd`]).
pub fn apply_energy_decay_to_zone(zf: &mut ZoneFile, factor_q16: u16, high_water: u32) {
    let zone_slice = zf.as_slice_mut();
    let stride = NodeRecord::SIZE;
    // high_water is exclusive; slot 0 is reserved.
    let first_slot: u32 = 1;
    let last_slot_exclusive: u32 = high_water;
    let total_capacity = zone_slice.len() / stride;
    let cap = (last_slot_exclusive as usize).min(total_capacity);
    let mut slot = first_slot as usize;

    // --- Batched SIMD body: 8 slots per iteration ------------------------
    while slot + 8 <= cap {
        let base = slot * stride;
        // Gather 8 u16 energies + 8 flags. We go through bytemuck on each
        // slot to stay strict-aliasing-safe under the unsafe_code ban at
        // the crate root — the inner `decay_u16x8` is the only unsafe site.
        let mut lanes = [0u16; 8];
        let mut alive = [false; 8];
        for i in 0..8 {
            let off = base + i * stride;
            let rec: &NodeRecord = bytemuck::from_bytes(&zone_slice[off..off + stride]);
            lanes[i] = rec.energy;
            alive[i] = rec.flags & crate::record::node_flags::TOMBSTONED == 0;
        }
        crate::simd::decay_u16x8(&mut lanes, factor_q16);
        for i in 0..8 {
            if !alive[i] {
                continue;
            }
            let off = base + i * stride;
            let rec: &mut NodeRecord = bytemuck::from_bytes_mut(&mut zone_slice[off..off + stride]);
            rec.energy = lanes[i];
        }
        slot += 8;
    }

    // --- Scalar tail for the last (<8) slots -----------------------------
    let factor = factor_q16 as u32;
    while slot < cap {
        let offset = slot * stride;
        let rec: &mut NodeRecord =
            bytemuck::from_bytes_mut(&mut zone_slice[offset..offset + stride]);
        if rec.flags & crate::record::node_flags::TOMBSTONED == 0 {
            rec.energy = (((rec.energy as u32) * factor) >> 16) as u16;
        }
        slot += 1;
    }
}

/// Apply a synapse-decay step in place to every live edge slot in the zone.
///
/// `factor_q16` is a Q0.16 multiplier; the operation is:
///
/// ```text
///   weight_u16 ← (weight_u16 × factor_q16) / 65536
/// ```
///
/// Tombstoned slots (`edge_flags::TOMBSTONED`) are skipped. `high_water` is the
/// exclusive upper bound of allocated edge slots (slot 0 is reserved). The
/// function is side-effect only — no WAL logging; callers own the WAL.
///
/// Design note: the column-view model treats every edge as a potential
/// synapse (the weight column lives on `EdgeRecord`), so this decays *all*
/// live edges rather than filtering on a specific edge_type. Non-synapse
/// edges with weight 0 decay to 0 — a no-op. If a future iteration wants
/// to restrict decay to `edge_flags::SYNAPSE_ACTIVE`, that's an O(1) flag
/// test inside this loop.
///
/// The hot path batches 8 edge slots per iteration through
/// [`crate::simd::decay_u16x8`], gathering/scattering `weight_u16` through a
/// stack buffer and masking tombstoned slots on write-back. Bit-for-bit
/// equivalent to the scalar fallback — property-tested in [`crate::simd`].
pub fn apply_synapse_decay_to_zone(zf: &mut ZoneFile, factor_q16: u16, high_water: u64) {
    let zone_slice = zf.as_slice_mut();
    let stride = EdgeRecord::SIZE;
    let total_capacity = zone_slice.len() / stride;
    let cap = (high_water as usize).min(total_capacity);
    let mut slot: usize = 1;

    // --- Batched SIMD body: 8 edges per iteration ------------------------
    while slot + 8 <= cap {
        let base = slot * stride;
        let mut lanes = [0u16; 8];
        let mut alive = [false; 8];
        for i in 0..8 {
            let off = base + i * stride;
            let rec: &EdgeRecord = bytemuck::from_bytes(&zone_slice[off..off + stride]);
            lanes[i] = rec.weight_u16;
            alive[i] = rec.flags & crate::record::edge_flags::TOMBSTONED == 0;
        }
        crate::simd::decay_u16x8(&mut lanes, factor_q16);
        for i in 0..8 {
            if !alive[i] {
                continue;
            }
            let off = base + i * stride;
            let rec: &mut EdgeRecord = bytemuck::from_bytes_mut(&mut zone_slice[off..off + stride]);
            rec.weight_u16 = lanes[i];
        }
        slot += 8;
    }

    // --- Scalar tail for the last (<8) slots -----------------------------
    let factor = factor_q16 as u32;
    while slot < cap {
        let offset = slot * stride;
        let rec: &mut EdgeRecord =
            bytemuck::from_bytes_mut(&mut zone_slice[offset..offset + stride]);
        if rec.flags & crate::record::edge_flags::TOMBSTONED == 0 {
            rec.weight_u16 = (((rec.weight_u16 as u32) * factor) >> 16) as u16;
        }
        slot += 1;
    }
}

/// Apply a Q0.16 multiplicative decay to every live edge whose `edge_type`
/// equals `coact_type_id`.
///
/// Sibling to [`apply_synapse_decay_to_zone`] but type-filtered: synapse
/// edges and any other non-COACT edge type are skipped (their `weight_u16`
/// column is left untouched). The math itself is identical to the
/// synapse decay — `weight ← (weight × factor_q16) >> 16` — but applied
/// only to slots whose `edge_type` matches.
///
/// Tombstoned slots are skipped. Slot 0 is reserved.
///
/// The hot path batches 8 edge slots per iteration, gathering eight
/// `weight_u16`s and eight `(alive ∧ is_coact)` masks, running the
/// shared SIMD `decay_u16x8` kernel, then scattering only the masked
/// lanes back. Bit-for-bit equivalent to the scalar fallback for the
/// COACT-typed slots; non-COACT slots are bit-for-bit untouched.
pub fn apply_coact_decay_to_zone(
    zf: &mut ZoneFile,
    factor_q16: u16,
    high_water: u64,
    coact_type_id: u16,
) {
    let zone_slice = zf.as_slice_mut();
    let stride = EdgeRecord::SIZE;
    let total_capacity = zone_slice.len() / stride;
    let cap = (high_water as usize).min(total_capacity);
    let mut slot: usize = 1;

    // --- Batched SIMD body: 8 edges per iteration ------------------------
    while slot + 8 <= cap {
        let base = slot * stride;
        let mut lanes = [0u16; 8];
        let mut decay_lane = [false; 8];
        for i in 0..8 {
            let off = base + i * stride;
            let rec: &EdgeRecord = bytemuck::from_bytes(&zone_slice[off..off + stride]);
            lanes[i] = rec.weight_u16;
            decay_lane[i] = rec.flags & crate::record::edge_flags::TOMBSTONED == 0
                && rec.edge_type == coact_type_id;
        }
        crate::simd::decay_u16x8(&mut lanes, factor_q16);
        for i in 0..8 {
            if !decay_lane[i] {
                continue;
            }
            let off = base + i * stride;
            let rec: &mut EdgeRecord = bytemuck::from_bytes_mut(&mut zone_slice[off..off + stride]);
            rec.weight_u16 = lanes[i];
        }
        slot += 8;
    }

    // --- Scalar tail for the last (<8) slots -----------------------------
    let factor = factor_q16 as u32;
    while slot < cap {
        let offset = slot * stride;
        let rec: &mut EdgeRecord =
            bytemuck::from_bytes_mut(&mut zone_slice[offset..offset + stride]);
        if rec.flags & crate::record::edge_flags::TOMBSTONED == 0 && rec.edge_type == coact_type_id
        {
            rec.weight_u16 = (((rec.weight_u16 as u32) * factor) >> 16) as u16;
        }
        slot += 1;
    }
}

/// Summary of a single [`Writer::compact_community`] transaction.
///
/// * `community_id` — the community that was relocated.
/// * `relocations` — `(old_slot, new_slot)` pairs describing every live node
///   that was moved. Length = number of live members of the community at the
///   time of the call.
/// * `new_node_hw` — post-compaction slot high-water mark the caller must
///   publish to the store's slot allocator so subsequent inserts land past
///   the relocated range.
/// * `page_range` — the `(first_page, last_page)` half-open interval the
///   relocated nodes now occupy, for verification / debugging.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub community_id: u32,
    pub relocations: Vec<(u32, u32)>,
    pub new_node_hw: u32,
    pub page_range: (u32, u32),
}

impl CompactionResult {
    /// Empty / no-op outcome (used when the target community has no live
    /// members, so there's nothing to relocate).
    pub fn empty() -> Self {
        Self {
            community_id: 0,
            relocations: Vec::new(),
            new_node_hw: 0,
            page_range: (0, 0),
        }
    }

    /// `true` when no nodes were moved.
    pub fn is_empty(&self) -> bool {
        self.relocations.is_empty()
    }
}

// Point d'injection de panne pour `ensure_room` — **compile uniquement
// sous `cfg(test)`**, donc rigoureusement inexistant en production.
//
// Il existe parce que la fenetre « annonce posee, aucune zone touchee »
// n'est atteignable QUE par un echec de croissance de zone (ENOSPC,
// echec de remap mmap) : elle est indeclenchable depuis l'API publique.
// Sans lui, le test du point 2 ne pourrait qu'appeler directement le
// helper de nettoyage — c'est-a-dire se tester lui-meme.
//
// Deux proprietes qui le rendent sur :
// * `cfg(test)` — aucune surface en production, contrairement a une
//   garde `pub` qu'un appelant pourrait satisfaire artificiellement ;
// * `thread_local` — chaque `#[test]` a son propre thread, l'armement ne
//   peut pas empoisonner les tests qui tournent en parallele.
//
// Valeur : `Some((suffixe, sauts))` laisse passer `sauts` appels dont le
// chemin de zone se termine par `suffixe`, fait echouer le suivant, puis
// se desarme. Le compteur de sauts est indispensable pour viser une
// fenetre PRECISE : la zone d'epoch est touchee deux fois par une passe
// de repermutation (l'ANNONCE, avant les mutations, puis la
// CONFIRMATION, apres la barriere), et ces deux echecs appellent des
// reprises OPPOSEES.
#[cfg(test)]
thread_local! {
    pub(crate) static ECHEC_ENSURE_ROOM: std::cell::RefCell<Option<(String, u32)>> =
        const { std::cell::RefCell::new(None) };
}

// Failpoint jumeau du precedent, mais sur `msync` — donc APRES que la
// page a ete ecrite et AVANT que sa durabilite ne soit acquise.
//
// Il existe parce que le failpoint `ensure_room` ne pouvait pas atteindre
// la fenetre trouvee en revue #6 : `write_epoch_state` ecrit la page PUIS
// demande sa durabilite, et c'est un echec A CETTE COUTURE qui laisse
// `pending` sur le disque pendant que l'annonce echoue. Un failpoint pose
// trop tot rend le test VIDE — ce qu'une mutation a montre.
#[cfg(test)]
thread_local! {
    pub(crate) static ECHEC_MSYNC: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Arme le failpoint de `msync` : le prochain `ZoneFile::msync` sur une
/// zone dont le chemin se termine par `suffixe` echouera, puis se desarme.
#[cfg(test)]
pub(crate) fn armer_echec_msync(suffixe: &str) {
    ECHEC_MSYNC.with(|c| *c.borrow_mut() = Some(suffixe.to_string()));
}

/// Vrai si le failpoint de `msync` a TIRE (donc s'est desarme).
#[cfg(test)]
pub(crate) fn echec_msync_a_tire() -> bool {
    ECHEC_MSYNC.with(|c| c.borrow().is_none())
}

/// Arme le failpoint : le `sauts`-ieme appel a `ensure_room` sur une zone
/// dont le chemin se termine par `suffixe` echouera (0 = le prochain).
#[cfg(test)]
pub(crate) fn armer_echec_ensure_room(suffixe: &str, sauts: u32) {
    ECHEC_ENSURE_ROOM.with(|c| *c.borrow_mut() = Some((suffixe.to_string(), sauts)));
}

/// Vrai si le failpoint a TIRE (donc s'est desarme). Une assertion
/// anti-vacuite a part entiere : sans elle, un test qui echouerait avant
/// d'atteindre la fenetre visee passerait au vert sans rien couvrir.
#[cfg(test)]
pub(crate) fn echec_ensure_room_a_tire() -> bool {
    ECHEC_ENSURE_ROOM.with(|c| c.borrow().is_none())
}

/// Ensure `zf` is at least `needed_end` bytes long, with exponential pre-alloc.
///
/// Growth policy: target = max(needed_end + min_headroom, current × 3/2),
/// rounded up to 4 KiB. This yields O(log N) remaps for N-byte bulk inserts.
pub fn ensure_room(zf: &mut ZoneFile, needed_end: usize, min_headroom: u64) -> SubstrateResult<()> {
    #[cfg(test)]
    {
        let vise = ECHEC_ENSURE_ROOM.with(|c| {
            let mut m = c.borrow_mut();
            let Some((suffixe, sauts)) = m.as_mut() else {
                return false;
            };
            if !zf.path().to_string_lossy().ends_with(suffixe.as_str()) {
                return false;
            }
            if *sauts > 0 {
                *sauts -= 1;
                return false;
            }
            *m = None; // desarmement : le test peut ASSERTER qu'il a tire
            true
        });
        if vise {
            return Err(SubstrateError::Io(std::io::Error::other(
                "ENOSPC simule (failpoint de test) : croissance de zone refusee",
            )));
        }
    }
    let need = needed_end as u64;
    if zf.len() >= need {
        return Ok(());
    }
    let grown = (zf.len() * 3 / 2).max(need + min_headroom);
    let aligned = grown.div_ceil(4096) * 4096;
    zf.grow_to(aligned)?;
    Ok(())
}

fn unix_micros() -> i64 {
    use web_time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{PackedScarUtilAff, U48, f32_to_q1_15};

    fn sample_node(i: u32) -> NodeRecord {
        NodeRecord {
            label_bitset: 1 << (i % 64),
            first_edge_off: U48::from_u64(i as u64 * 32),
            first_prop_off: U48::from_u64(i as u64 * 16),
            community_id: i / 100,
            energy: f32_to_q1_15(0.5),
            scar_util_affinity: PackedScarUtilAff::new(1, 2, 3, true).pack(),
            centrality_cached: i as u16,
            flags: 0,
            // (jalon 2) appartenance : fixture publique.
            tenant_id: 0,
            _reserve: 0,
        }
    }

    fn sample_edge(i: u64) -> EdgeRecord {
        EdgeRecord {
            src: i as u32,
            dst: (i + 1) as u32,
            edge_type: (i as u16) % 10,
            weight_u16: f32_to_q1_15(0.3),
            next_from: U48::from_u64(i * 32),
            next_to: U48::from_u64(i * 64),
            first_prop_off: U48::ZERO,
            ricci_u8: (i % 256) as u8,
            flags: 0,
            engram_tag: i as u16,
            _pad: [0; 2],
        }
    }

    #[test]
    fn hopfield_recall_returns_top_k_by_bitset_overlap() {
        // T7 Step 3 — Writer end-to-end. Three co-engram clusters:
        //   - nodes 1,2,3,4 all belong to engrams 1 and 2 (bits 1 and 2 set)
        //   - nodes 5,6 belong to engrams 1 and 3 (bits 1 and 3 set)
        //   - nodes 7,8 belong to engram 4 only (bit 4 set)
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        for nid in [1u32, 2, 3, 4] {
            w.add_engram_bit(nid, 1).unwrap();
            w.add_engram_bit(nid, 2).unwrap();
        }
        for nid in [5u32, 6] {
            w.add_engram_bit(nid, 1).unwrap();
            w.add_engram_bit(nid, 3).unwrap();
        }
        for nid in [7u32, 8] {
            w.add_engram_bit(nid, 4).unwrap();
        }
        w.commit().unwrap();

        // Query from node 1 (bitset = bit1 | bit2). Expect 2, 3, 4 first
        // (overlap=2 — bits 1 and 2), then 5, 6 (overlap=1 — bit 1 only),
        // then nothing from 7, 8 (overlap=0).
        let got = w.hopfield_recall(1, 10).unwrap();
        let ids: Vec<u32> = got.iter().map(|(nid, _)| *nid).collect();
        // 1 itself is returned (full self-overlap = 2).
        assert_eq!(ids[0], 1);
        // Next three must be 2, 3, 4 (overlap=2, ordered by ascending idx).
        assert_eq!(&ids[1..4], &[2, 3, 4]);
        // Then 5, 6 (overlap=1).
        assert_eq!(&ids[4..6], &[5, 6]);
        // Nothing from 7, 8.
        for (nid, _) in got.iter() {
            assert!(*nid != 7 && *nid != 8, "non-overlapping nodes leaked");
        }
        // Overlap values are monotonically non-increasing.
        for w in got.windows(2) {
            assert!(w[0].1 >= w[1].1);
        }
    }

    #[test]
    fn hopfield_recall_empty_query_yields_empty() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        // Node 99 has never been given any engram bit.
        let got = w.hopfield_recall(99, 10).unwrap();
        assert!(got.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn hopfield_recall_k_zero_yields_empty() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        w.add_engram_bit(1, 1).unwrap();
        w.add_engram_bit(2, 1).unwrap();
        let got = w.hopfield_recall(1, 0).unwrap();
        assert!(got.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn write_node_then_read_back_from_mmap() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        for i in 0..10u32 {
            w.write_node(i, sample_node(i)).unwrap();
        }
        w.commit().unwrap();
        w.msync_zones().unwrap();

        // Read back via the mmap view.
        let zones = w.zones.read();
        let zf = zones.nodes.as_ref().unwrap();
        let bytes = zf.as_slice();
        let slice: &[NodeRecord] = bytemuck::cast_slice(&bytes[..10 * NodeRecord::SIZE]);
        for i in 0..10u32 {
            // (jalon 1) le writer pose ALLOCATED sur tout record ecrit.
            let mut attendu = sample_node(i);
            attendu.flags |= crate::record::node_flags::ALLOCATED;
            assert_eq!(slice[i as usize], attendu);
        }
    }

    /// (2026-08-07) Prouve que DEUX lectures tiennent la zone en même temps.
    ///
    /// Avec l'ancien `Mutex<ZoneCache>`, la seconde lecture ne pouvait pas
    /// entrer tant que la première tenait le verrou : ce test s'arrêtait au
    /// bout des 5 s d'attente. C'est le garde-fou qui empêche de revenir
    /// silencieusement à un verrou exclusif sur le chemin le plus chaud du
    /// moteur.
    #[test]
    fn deux_lectures_tiennent_la_zone_en_meme_temps() {
        use std::sync::mpsc;
        use std::time::Duration;

        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Arc::new(Writer::new(sub, SyncMode::Never).unwrap());
        w.write_node(0, sample_node(0)).unwrap();
        w.commit().unwrap();
        // Ouvre la zone : les deux lecteurs passeront par le chemin `.read()`
        // et non par l'ouverture paresseuse, qui exige `.write()`.
        w.read_node(0).unwrap();

        let (tenu_tx, tenu_rx) = mpsc::channel();
        let (relache_tx, relache_rx) = mpsc::channel();
        let w1 = Arc::clone(&w);
        let premier = std::thread::spawn(move || {
            w1.with_node_zone_read(|_zf| {
                tenu_tx.send(()).unwrap();
                relache_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });

        // Le premier lecteur tient maintenant le verrou et ne le rendra pas
        // avant qu'on le lui dise.
        tenu_rx.recv().unwrap();

        let (fini_tx, fini_rx) = mpsc::channel();
        let w2 = Arc::clone(&w);
        std::thread::spawn(move || {
            let r = w2.read_node(0);
            let _ = fini_tx.send(r.is_ok() && r.unwrap().is_some());
        });

        let second = fini_rx.recv_timeout(Duration::from_secs(5));
        relache_tx.send(()).unwrap();
        premier.join().unwrap();

        assert!(
            second.is_ok(),
            "la seconde lecture n'a pas pu entrer pendant que la première \
             tenait la zone : le verrou est redevenu exclusif"
        );
        assert!(second.unwrap(), "la seconde lecture n'a pas rendu le nœud");
    }

    #[test]
    fn write_edge_persists_via_zone_file() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        for i in 0..20u64 {
            w.write_edge(i, sample_edge(i)).unwrap();
        }
        w.commit().unwrap();
        w.msync_zones().unwrap();

        let zones = w.zones.read();
        let zf = zones.edges.as_ref().unwrap();
        let bytes = zf.as_slice();
        let slice: &[EdgeRecord] = bytemuck::cast_slice(&bytes[..20 * EdgeRecord::SIZE]);
        for i in 0..20u64 {
            assert_eq!(slice[i as usize], sample_edge(i));
        }
    }

    #[test]
    fn wal_contains_every_mutation() {
        // Use a persistent tempdir (kept alive until end-of-test) so the WAL
        // file survives the Writer's drop — open_tempfile() deletes the dir
        // when the SubstrateFile inside the Writer drops.
        let td = tempfile::tempdir().unwrap();
        let sub = SubstrateFile::create(td.path().join("kb")).unwrap();
        let wal_path = sub.wal_path();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        for i in 0..5u32 {
            w.write_node(i, sample_node(i)).unwrap();
        }
        w.commit().unwrap();
        w.wal().fsync().unwrap();
        drop(w);

        let r = crate::wal_io::WalReader::open(&wal_path).unwrap();
        let items: Vec<_> = r.iter_from(0).collect::<Result<Vec<_>, _>>().unwrap();
        // 5 NodeInsertV3 (trames tenant, jalon 2) + 1 NoOp commit
        assert_eq!(items.len(), 6);
        assert!(matches!(
            items[0].0.payload,
            WalPayload::NodeInsertV3 { .. }
        ));
        assert!(items.last().unwrap().0.is_commit());
    }

    #[test]
    fn ensure_room_grows_exponentially() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let mut zf = sub.open_zone(Zone::Nodes).unwrap();
        let start = zf.len();
        // (jalon 2) la zone nait a 40 o (sentinelle de stride) : demander
        // UN octet de plus force la premiere croissance.
        ensure_room(&mut zf, start as usize + 1, 4096).unwrap();
        let after_small = zf.len();
        assert!(after_small >= 4096);
        ensure_room(&mut zf, (after_small as usize) + 1, 4096).unwrap();
        let after_grow = zf.len();
        assert!(after_grow > after_small);
        assert!(
            after_grow >= after_small * 3 / 2,
            "grow should be exponential: {start} → {after_small} → {after_grow}"
        );
    }

    #[test]
    fn sync_mode_every_commit_fsyncs_on_commit() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        for i in 0..3u32 {
            w.write_node(i, sample_node(i)).unwrap();
        }
        // commit() writes a FLAG_COMMIT record; EveryCommit fsyncs it.
        w.commit().unwrap();
    }

    // =====================================================================
    // T6 cognitive-column primitives
    // =====================================================================

    #[test]
    fn reinforce_energy_mutates_column_and_logs_wal() {
        let td = tempfile::tempdir().unwrap();
        let sub = SubstrateFile::create(td.path().join("kb")).unwrap();
        let wal_path = sub.wal_path();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        // Seed slot 1 as a real node so the column has a known prior state.
        w.write_node(1, sample_node(1)).unwrap();
        // Reinforce to 0.95.
        let new_q = f32_to_q1_15(0.95);
        w.reinforce_energy(1, new_q).unwrap();
        w.commit().unwrap();
        w.wal().fsync().unwrap();

        // Mmap reflects the new value.
        let got = w.read_node(1).unwrap().unwrap();
        assert_eq!(got.energy, new_q);

        // WAL stream contains the dedicated record.
        drop(w);
        let r = crate::wal_io::WalReader::open(&wal_path).unwrap();
        let items: Vec<_> = r.iter_from(0).collect::<Result<Vec<_>, _>>().unwrap();
        let has_reinforce = items
            .iter()
            .any(|(rec, _, _)| matches!(rec.payload, WalPayload::EnergyReinforce { .. }));
        assert!(has_reinforce, "EnergyReinforce record missing from WAL");
    }

    #[test]
    fn update_node_first_prop_off_mutates_only_the_head_pointer() {
        // T17c Step 3b.2: setting the first_prop_off head must touch the 6
        // target bytes and nothing else, and must land a dedicated WAL record.
        let td = tempfile::tempdir().unwrap();
        let kb = td.path().join("kb");
        let sub = SubstrateFile::create(&kb).unwrap();
        let wal_path = sub.wal_path();
        let w = Writer::new(sub, SyncMode::Never).unwrap();

        // Seed slot 1 with a full known state.
        let mut seed = sample_node(1);
        seed.energy = 0x7FFF;
        seed.scar_util_affinity = 0xBEEF;
        seed.centrality_cached = 0xABCD;
        seed.community_id = 42;
        seed.flags = crate::record::node_flags::CENTRALITY_STALE;
        seed.first_edge_off = U48::from_u64(0x00_1234_5678);
        w.write_node(1, seed).unwrap();
        w.commit().unwrap();

        // Now move the head pointer.
        let new_head = U48::from_u64(0xAA_BBCC_DDEE); // fits in 48 bits
        w.update_node_first_prop_off(1, new_head).unwrap();
        w.commit().unwrap();
        w.wal().fsync().unwrap();

        // Readback: every unrelated field preserved, first_prop_off flipped.
        let got = w.read_node(1).unwrap().unwrap();
        assert_eq!(got.first_prop_off, new_head, "head pointer must be updated");
        assert_eq!(got.energy, 0x7FFF, "energy must be preserved");
        assert_eq!(got.scar_util_affinity, 0xBEEF);
        assert_eq!(got.centrality_cached, 0xABCD);
        assert_eq!(got.community_id, 42);
        assert_eq!(
            got.flags,
            // (jalon 1) ALLOCATED pose a l'ecriture s'ajoute aux flags seedes.
            crate::record::node_flags::CENTRALITY_STALE | crate::record::node_flags::ALLOCATED
        );
        assert_eq!(got.first_edge_off, U48::from_u64(0x00_1234_5678));
        assert_eq!(got.label_bitset, sample_node(1).label_bitset);

        // WAL contains exactly one NodePropHeadUpdate with the right node+head.
        drop(w);
        let r = crate::wal_io::WalReader::open(&wal_path).unwrap();
        let items: Vec<_> = r.iter_from(0).collect::<Result<Vec<_>, _>>().unwrap();
        let matches: Vec<_> = items
            .iter()
            .filter_map(|(rec, _, _)| match &rec.payload {
                WalPayload::NodePropHeadUpdate {
                    node_id,
                    first_prop_off,
                } => Some((*node_id, *first_prop_off)),
                _ => None,
            })
            .collect();
        assert_eq!(matches.len(), 1, "exactly one NodePropHeadUpdate expected");
        assert_eq!(matches[0], (1, new_head.to_u64()));
    }

    #[test]
    fn update_node_first_prop_off_replays_after_mmap_wipe() {
        // T17c Step 3b.2: simulate a crash between WAL-append and mmap flush
        // by wiping the nodes zone then replaying from the WAL.
        let td = tempfile::tempdir().unwrap();
        let kb = td.path().join("kb");
        let sub = SubstrateFile::create(&kb).unwrap();
        let nodes_zone_path = kb.join(crate::file::zone::NODES);
        let w = Writer::new(sub, SyncMode::Never).unwrap();

        // Seed + update head pointer, commit, fsync WAL (durable), but
        // intentionally SKIP msync_zones (simulates crash before flush).
        w.write_node(1, sample_node(1)).unwrap();
        let new_head = U48::from_u64(0x00_0001_0200); // known U48 value
        w.update_node_first_prop_off(1, new_head).unwrap();
        w.commit().unwrap();
        w.wal().fsync().unwrap();
        drop(w);

        // Wipe the nodes zone on disk so the mmap state for slot 1 is gone.
        // The zone file is pre-created as a 0-byte file by SubstrateFile::create,
        // so truncating it is enough to simulate the crash.
        if nodes_zone_path.exists() {
            std::fs::write(&nodes_zone_path, []).unwrap();
        }

        // Reopen + replay.
        let sub2 = SubstrateFile::open(&kb).unwrap();
        let _replay_stats = crate::replay::replay_from(&sub2, 0).unwrap();
        drop(sub2);

        let sub3 = SubstrateFile::open(&kb).unwrap();
        let w3 = Writer::new(sub3, SyncMode::Never).unwrap();
        let got = w3.read_node(1).unwrap().unwrap();
        assert_eq!(
            got.first_prop_off, new_head,
            "head pointer must survive crash + replay"
        );
    }

    #[test]
    fn update_edge_first_prop_off_mutates_only_the_head_pointer() {
        // T17f Step 3: mirror of `update_node_first_prop_off_mutates_only_the_head_pointer`
        // for edges. Setting the first_prop_off head must touch the 6 target
        // bytes at offset 24 within the EdgeRecord and nothing else, and
        // must land a dedicated WAL record.
        let td = tempfile::tempdir().unwrap();
        let kb = td.path().join("kb");
        let sub = SubstrateFile::create(&kb).unwrap();
        let wal_path = sub.wal_path();
        let w = Writer::new(sub, SyncMode::Never).unwrap();

        // Seed slot 1 with a full known state (every mutable field != default).
        let mut seed = sample_edge(1);
        seed.weight_u16 = 0x4321;
        seed.ricci_u8 = 0xA5;
        seed.flags = crate::record::edge_flags::SYNAPSE_ACTIVE;
        seed.engram_tag = 0xBEEF;
        seed.next_from = U48::from_u64(0x00_1234_5678);
        seed.next_to = U48::from_u64(0x00_9ABC_DEF0);
        w.write_edge(1, seed).unwrap();
        w.commit().unwrap();

        // Now move the head pointer.
        let new_head = U48::from_u64(0xAA_BBCC_DDEE); // fits in 48 bits
        w.update_edge_first_prop_off(1, new_head).unwrap();
        w.commit().unwrap();
        w.wal().fsync().unwrap();

        // Readback: every unrelated field preserved, first_prop_off flipped.
        let got = w.read_edge(1).unwrap().unwrap();
        assert_eq!(got.first_prop_off, new_head, "head pointer must be updated");
        assert_eq!(got.src, 1);
        assert_eq!(got.dst, 2);
        assert_eq!(got.edge_type, 1); // (1 as u16) % 10
        assert_eq!(got.weight_u16, 0x4321);
        assert_eq!(got.ricci_u8, 0xA5);
        assert_eq!(got.flags, crate::record::edge_flags::SYNAPSE_ACTIVE);
        assert_eq!(got.engram_tag, 0xBEEF);
        assert_eq!(got.next_from, U48::from_u64(0x00_1234_5678));
        assert_eq!(got.next_to, U48::from_u64(0x00_9ABC_DEF0));

        // WAL contains exactly one EdgePropHeadUpdate with the right edge+head.
        drop(w);
        let r = crate::wal_io::WalReader::open(&wal_path).unwrap();
        let items: Vec<_> = r.iter_from(0).collect::<Result<Vec<_>, _>>().unwrap();
        let matches: Vec<_> = items
            .iter()
            .filter_map(|(rec, _, _)| match &rec.payload {
                WalPayload::EdgePropHeadUpdate {
                    edge_idx,
                    first_prop_off,
                } => Some((*edge_idx, *first_prop_off)),
                _ => None,
            })
            .collect();
        assert_eq!(matches.len(), 1, "exactly one EdgePropHeadUpdate expected");
        assert_eq!(matches[0], (1, new_head.to_u64()));
    }

    #[test]
    fn update_edge_first_prop_off_replays_after_mmap_wipe() {
        // T17f Step 3: mirror of `update_node_first_prop_off_replays_after_mmap_wipe`
        // for edges. Simulate a crash between WAL-append and mmap flush by
        // wiping the edges zone then replaying from the WAL.
        let td = tempfile::tempdir().unwrap();
        let kb = td.path().join("kb");
        let sub = SubstrateFile::create(&kb).unwrap();
        let edges_zone_path = kb.join(crate::file::zone::EDGES);
        let w = Writer::new(sub, SyncMode::Never).unwrap();

        // Seed + update head pointer, commit, fsync WAL (durable), but
        // intentionally SKIP msync_zones (simulates crash before flush).
        w.write_edge(1, sample_edge(1)).unwrap();
        let new_head = U48::from_u64(0x00_0001_0200); // known U48 value
        w.update_edge_first_prop_off(1, new_head).unwrap();
        w.commit().unwrap();
        w.wal().fsync().unwrap();
        drop(w);

        // Wipe the edges zone on disk so the mmap state for slot 1 is gone.
        if edges_zone_path.exists() {
            std::fs::write(&edges_zone_path, []).unwrap();
        }

        // Reopen + replay.
        let sub2 = SubstrateFile::open(&kb).unwrap();
        let _replay_stats = crate::replay::replay_from(&sub2, 0).unwrap();
        drop(sub2);

        let sub3 = SubstrateFile::open(&kb).unwrap();
        let w3 = Writer::new(sub3, SyncMode::Never).unwrap();
        let got = w3.read_edge(1).unwrap().unwrap();
        assert_eq!(
            got.first_prop_off, new_head,
            "edge head pointer must survive crash + replay"
        );
    }

    #[test]
    fn decay_all_energy_multiplies_every_live_slot() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        // Write 5 nodes with known energy = 0.8.
        for i in 1..=5u32 {
            let mut n = sample_node(i);
            n.set_energy_f32(0.8);
            w.write_node(i, n).unwrap();
        }
        // Tombstone slot 3 — must NOT be touched by decay.
        w.tombstone_node(3).unwrap();
        w.commit().unwrap();

        // Apply ×0.5 decay.
        w.decay_all_energy(32768, 6).unwrap();
        w.commit().unwrap();

        for i in 1..=5u32 {
            let rec = w.read_node(i).unwrap().unwrap();
            if i == 3 {
                // Tombstoned: energy preserved, flags flipped.
                assert!(rec.is_tombstoned());
            } else {
                let expected = f32_to_q1_15(0.4); // 0.8 × 0.5
                // Q0.16 factor 32768 / 65536 is 0.5 exactly; accept ±1 ULP.
                let delta = (rec.energy as i32 - expected as i32).abs();
                assert!(
                    delta <= 1,
                    "slot {i}: energy={} expected≈{expected}",
                    rec.energy
                );
            }
        }
    }

    #[test]
    fn set_scar_util_affinity_roundtrip() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        w.write_node(7, sample_node(7)).unwrap();
        let packed = PackedScarUtilAff::new(11, 22, 30, true).pack();
        w.set_scar_util_affinity(7, packed).unwrap();
        w.commit().unwrap();
        let rec = w.read_node(7).unwrap().unwrap();
        let u = PackedScarUtilAff::unpack(rec.scar_util_affinity);
        assert_eq!(u.scar, 11);
        assert_eq!(u.utility, 22);
        assert_eq!(u.affinity, 30);
        assert!(u.dirty);
    }

    #[test]
    fn decay_all_synapse_multiplies_every_live_edge() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        // Seed 5 edges with known weight = 0.8 (in Q0.16).
        let seed = (0.8_f32 * 65535.0).round() as u16;
        for i in 1..=5u64 {
            let mut e = sample_edge(i);
            e.weight_u16 = seed;
            w.write_edge(i, e).unwrap();
        }
        // Tombstone slot 3 — must NOT be touched.
        w.tombstone_edge(3).unwrap();
        w.commit().unwrap();

        // Apply ×0.5 decay.
        w.decay_all_synapse(32768, 6).unwrap();
        w.commit().unwrap();

        for i in 1..=5u64 {
            let rec = w.read_edge(i).unwrap().unwrap();
            if i == 3 {
                assert!(rec.is_tombstoned());
            } else {
                let expected = (seed as u32 * 32768) >> 16;
                let delta = (rec.weight_u16 as i32 - expected as i32).abs();
                assert!(
                    delta <= 1,
                    "slot {i}: weight={} expected≈{expected}",
                    rec.weight_u16
                );
            }
        }
    }

    #[test]
    fn reinforce_synapse_updates_weight_column() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        w.write_edge(4, sample_edge(4)).unwrap();
        let new_w = (0.77_f32.clamp(0.0, 1.0) * 65535.0).round() as u16;
        w.reinforce_synapse(4, new_w).unwrap();
        w.commit().unwrap();
        let rec = w.read_edge(4).unwrap().unwrap();
        assert_eq!(rec.weight_u16, new_w);
    }

    #[test]
    fn update_scar_field_preserves_other_fields() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        w.write_node(1, sample_node(1)).unwrap();
        // Seed all three sub-fields via whole-column write.
        w.set_scar_util_affinity(1, PackedScarUtilAff::new(3, 7, 15, false).pack())
            .unwrap();
        // Update only the scar field.
        w.update_scar_field(1, 25).unwrap();
        w.commit().unwrap();
        let rec = w.read_node(1).unwrap().unwrap();
        let u = PackedScarUtilAff::unpack(rec.scar_util_affinity);
        assert_eq!(u.scar, 25, "scar updated");
        assert_eq!(u.utility, 7, "utility preserved");
        assert_eq!(u.affinity, 15, "affinity preserved");
        assert!(u.dirty, "dirty forced to 1");
    }

    #[test]
    fn update_utility_and_affinity_fields_are_orthogonal() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        w.write_node(2, sample_node(2)).unwrap();
        // Zero-seed scar to distinguish from the sample_node default.
        w.set_scar_util_affinity(2, PackedScarUtilAff::new(0, 0, 0, false).pack())
            .unwrap();
        w.update_utility_field(2, 19).unwrap();
        w.update_affinity_field(2, 11).unwrap();
        w.commit().unwrap();
        let rec = w.read_node(2).unwrap().unwrap();
        let u = PackedScarUtilAff::unpack(rec.scar_util_affinity);
        assert_eq!(u.scar, 0, "scar not touched by util/affinity updates");
        assert_eq!(u.utility, 19);
        assert_eq!(u.affinity, 11);
        assert!(u.dirty);
    }

    // =====================================================================
    // T7 Step 0 — Engram membership side-table
    // =====================================================================

    #[test]
    fn set_engram_members_walks_and_mutates_zone() {
        let td = tempfile::tempdir().unwrap();
        let sub = SubstrateFile::create(td.path().join("kb")).unwrap();
        let wal_path = sub.wal_path();
        let w = Writer::new(sub, SyncMode::Never).unwrap();

        let members: Vec<u32> = (1..=100).map(|i| i * 3).collect();
        w.set_engram_members(42, members.clone()).unwrap();
        w.commit().unwrap();
        w.wal().fsync().unwrap();

        // (1) Direct readback from the side-table via the Writer.
        let got = w.engram_members(42).unwrap().unwrap();
        assert_eq!(got, members);

        // (2) WAL stream contains the EngramMembersSet payload.
        drop(w);
        let r = crate::wal_io::WalReader::open(&wal_path).unwrap();
        let items: Vec<_> = r.iter_from(0).collect::<Result<Vec<_>, _>>().unwrap();
        let found = items.iter().any(|(rec, _, _)| {
            matches!(
                &rec.payload,
                WalPayload::EngramMembersSet { engram_id: 42, members: m } if m == &members
            )
        });
        assert!(found, "EngramMembersSet record missing from WAL");
    }

    #[test]
    fn set_engram_members_close_reopen_roundtrip() {
        // T7 Step 0 acceptance: create engram with 100 members → close →
        // reopen → members() returns the same list, rebuilt from the WAL.
        let td = tempfile::tempdir().unwrap();
        let sub_path = td.path().join("kb");
        let members: Vec<u32> = (1..=100).map(|i| i * 7 + 1).collect();
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            w.set_engram_members(999, members.clone()).unwrap();
            w.commit().unwrap();
            w.msync_zones().unwrap();
            w.wal().fsync().unwrap();
        }
        // Re-open the substrate — mmap picks up the on-disk zone directly.
        let sub2 = SubstrateFile::open(&sub_path).unwrap();
        let w2 = Writer::new(sub2, SyncMode::Never).unwrap();
        let got = w2.engram_members(999).unwrap().unwrap();
        assert_eq!(got, members);
    }

    #[test]
    fn engram_members_rebuild_from_wal_after_zone_wipe() {
        // Simulate a crash that destroys the side-table zone file but keeps
        // the WAL durable. Replay must reconstruct the membership exactly.
        let td = tempfile::tempdir().unwrap();
        let sub_path = td.path().join("kb");
        let members: Vec<u32> = (1..=100).map(|i| i + 10_000).collect();
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            w.set_engram_members(1234, members.clone()).unwrap();
            // Also set a second engram to check replay handles multiple records.
            w.set_engram_members(1, vec![1u32, 2, 3, 4, 5]).unwrap();
            w.commit().unwrap();
            w.wal().fsync().unwrap();
            // Intentionally do NOT msync_zones — pretend the mmap writes
            // never hit disk.
        }
        // Wipe the side-table zone.
        std::fs::write(
            sub_path.join(crate::file::zone::ENGRAM_MEMBERS),
            Vec::<u8>::new(),
        )
        .unwrap();

        // Replay reconstructs the side-table from the WAL.
        let sub2 = SubstrateFile::open(&sub_path).unwrap();
        let stats = crate::replay::replay_from(&sub2, 0).unwrap();
        assert_eq!(stats.decode_errors, 0);

        let w2 = Writer::new(sub2, SyncMode::Never).unwrap();
        let got = w2.engram_members(1234).unwrap().unwrap();
        assert_eq!(got, members);
        let got1 = w2.engram_members(1).unwrap().unwrap();
        assert_eq!(got1, vec![1u32, 2, 3, 4, 5]);
    }

    // =====================================================================
    // T7 Step 1 — Engram bitset column
    // =====================================================================

    #[test]
    fn set_engram_bitset_roundtrip_via_writer() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        w.set_engram_bitset(7, 0xFACE_B00C_0000_0001).unwrap();
        w.commit().unwrap();
        let got = w.engram_bitset(7).unwrap();
        assert_eq!(got, 0xFACE_B00C_0000_0001);
        // Unset slots return 0.
        assert_eq!(w.engram_bitset(8).unwrap(), 0);
    }

    #[test]
    fn add_engram_bit_ors_in_mask() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        w.add_engram_bit(1, 5).unwrap();
        w.add_engram_bit(1, 10).unwrap();
        w.add_engram_bit(1, 70).unwrap(); // 70 % 64 = 6
        w.commit().unwrap();
        let got = w.engram_bitset(1).unwrap();
        assert_eq!(got, (1u64 << 5) | (1u64 << 10) | (1u64 << 6));
    }

    #[test]
    fn add_engram_bit_rejects_zero_engram_id() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        let err = w.add_engram_bit(1, 0).unwrap_err();
        assert!(matches!(err, crate::SubstrateError::WalBadFrame(_)));
    }

    #[test]
    fn engram_bitset_wal_rebuild_after_zone_wipe() {
        // Crash-safety: wipe the bitset column file, replay rebuilds it.
        let td = tempfile::tempdir().unwrap();
        let sub_path = td.path().join("kb");
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            // Mix of set_engram_bitset (absolute) and add_engram_bit (RMW).
            w.set_engram_bitset(1, 0xAAAA_BBBB).unwrap();
            w.add_engram_bit(2, 3).unwrap();
            w.add_engram_bit(2, 3).unwrap(); // idempotent
            w.add_engram_bit(2, 67).unwrap(); // 67 % 64 = 3 (collides)
            w.add_engram_bit(2, 7).unwrap();
            w.set_engram_bitset(100, 0xFFFF_FFFF_FFFF_FFFF).unwrap();
            w.commit().unwrap();
            w.wal().fsync().unwrap();
        }
        // Wipe the bitset zone.
        std::fs::write(
            sub_path.join(crate::file::zone::ENGRAM_BITSET),
            Vec::<u8>::new(),
        )
        .unwrap();

        let sub2 = SubstrateFile::open(&sub_path).unwrap();
        let stats = crate::replay::replay_from(&sub2, 0).unwrap();
        assert_eq!(stats.decode_errors, 0);

        let w2 = Writer::new(sub2, SyncMode::Never).unwrap();
        assert_eq!(w2.engram_bitset(1).unwrap(), 0xAAAA_BBBB);
        assert_eq!(w2.engram_bitset(2).unwrap(), (1u64 << 3) | (1u64 << 7));
        assert_eq!(w2.engram_bitset(100).unwrap(), 0xFFFF_FFFF_FFFF_FFFF);
        assert_eq!(w2.engram_bitset(3).unwrap(), 0); // untouched
    }

    #[test]
    fn centrality_batch_updates_and_clears_stale_flag() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        for i in 1..=3u32 {
            let mut n = sample_node(i);
            n.flags |= crate::record::node_flags::CENTRALITY_STALE;
            w.write_node(i, n).unwrap();
        }
        w.update_centrality_batch(vec![(1, 100), (2, 200), (3, 300)])
            .unwrap();
        w.commit().unwrap();
        for (slot, expected) in [(1u32, 100u16), (2, 200), (3, 300)] {
            let rec = w.read_node(slot).unwrap().unwrap();
            assert_eq!(rec.centrality_cached, expected);
            assert_eq!(
                rec.flags & crate::record::node_flags::CENTRALITY_STALE,
                0,
                "CENTRALITY_STALE flag must be cleared"
            );
        }
    }

    // T10 Step 3 — CommunityAssign batch writer + in-memory read-back.
    //
    // Writes three nodes with community_id = 0, calls
    // `update_community_batch` to assign ids (11, 22, 33), commits, then
    // verifies the mmap zone reflects the new assignments.
    #[test]
    fn community_batch_assigns_and_persists() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        for i in 1..=3u32 {
            let mut n = sample_node(i);
            n.community_id = 0;
            w.write_node(i, n).unwrap();
        }
        w.update_community_batch(vec![(1, 11), (2, 22), (3, 33)])
            .unwrap();
        w.commit().unwrap();
        for (slot, expected) in [(1u32, 11u32), (2, 22), (3, 33)] {
            assert_eq!(w.read_node(slot).unwrap().unwrap().community_id, expected);
        }
    }

    // T10 Step 3 — WAL replay round-trip for CommunityAssign. After the
    // zone file is wiped, a reopen + replay must restore every assignment.
    // Exercises the replay handler added in `replay.rs`.
    #[test]
    fn community_batch_wal_replay_round_trip() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        // Round 1: create substrate, write nodes, assign communities, commit.
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            for i in 1..=3u32 {
                let mut n = sample_node(i);
                n.community_id = 0;
                w.write_node(i, n).unwrap();
            }
            w.update_community_batch(vec![(1, 11), (2, 22), (3, 33)])
                .unwrap();
            w.commit().unwrap();
        }

        // Round 2: wipe the nodes zone (keep the WAL).
        std::fs::write(sub_path.join(crate::file::zone::NODES), []).unwrap();

        // Round 3: reopen + replay. community_id must be restored.
        let sub2 = SubstrateFile::open(&sub_path).unwrap();
        let stats = crate::replay::replay_from(&sub2, 0).unwrap();
        assert_eq!(stats.decode_errors, 0);

        let nz = sub2.open_zone(crate::file::Zone::Nodes).unwrap();
        let slice: &[NodeRecord] = bytemuck::cast_slice(&nz.as_slice()[..4 * NodeRecord::SIZE]);
        assert_eq!(slice[1].community_id, 11);
        assert_eq!(slice[2].community_id, 22);
        assert_eq!(slice[3].community_id, 33);
    }

    // T10 Step 3 — empty batch is a no-op (no WAL record, no mmap writes).
    #[test]
    fn community_batch_empty_is_noop() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        w.write_node(1, sample_node(1)).unwrap();
        let before = w.read_node(1).unwrap().unwrap().community_id;
        w.update_community_batch(Vec::new()).unwrap();
        w.commit().unwrap();
        assert_eq!(w.read_node(1).unwrap().unwrap().community_id, before);
    }

    // =====================================================================
    // T7 Step 5 — COACT typed-edge column primitives
    // =====================================================================

    /// Helper: write an edge slot with a specific edge_type and weight,
    /// using `Writer::write_edge` so the WAL records it. Other fields are
    /// the bog-standard `sample_edge` defaults.
    fn write_typed_edge(w: &Writer, slot: u64, edge_type: u16, weight_u16: u16) {
        let mut e = sample_edge(slot);
        e.edge_type = edge_type;
        e.weight_u16 = weight_u16;
        w.write_edge(slot, e).unwrap();
    }

    #[test]
    fn coact_reinforce_at_saturates_and_persists() {
        // T7 Step 5 — `coact_reinforce_at` is a saturating-add on the
        // weight column, returning the new weight, with the absolute
        // post-state logged via SynapseReinforce so replay is idempotent.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();

        // Seed slot 1 as a COACT edge (type id 7) with weight 0.
        write_typed_edge(&w, 1, 7, 0);

        // Reinforce by 0x4000 four times: 0 → 0x4000 → 0x8000 → 0xC000 → 0xFFFF (sat).
        let new = w.coact_reinforce_at(1, 0x4000).unwrap();
        assert_eq!(new, 0x4000);
        let new = w.coact_reinforce_at(1, 0x4000).unwrap();
        assert_eq!(new, 0x8000);
        let new = w.coact_reinforce_at(1, 0x4000).unwrap();
        assert_eq!(new, 0xC000);
        // Saturation: 0xC000 + 0x4000 = 0x10000 → clamped to 0xFFFF.
        let new = w.coact_reinforce_at(1, 0x4000).unwrap();
        assert_eq!(new, 0xFFFF);
        // One more — already at ceiling.
        let new = w.coact_reinforce_at(1, 0x1234).unwrap();
        assert_eq!(new, 0xFFFF);
        w.commit().unwrap();

        // Read back: the column reflects the saturated value.
        let rec = w.read_edge(1).unwrap().unwrap();
        assert_eq!(rec.weight_u16, 0xFFFF);
        // Type and other fields are preserved.
        assert_eq!(rec.edge_type, 7);
    }

    #[test]
    fn decay_all_coact_only_touches_coact_typed_edges() {
        // T7 Step 5 — `decay_all_coact(factor, hw, type_id)` filters by
        // edge_type so synapse and other types are bit-for-bit
        // untouched. Mix three edge types in the same zone and assert.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        const COACT_ID: u16 = 7;
        const SYNAPSE_ID: u16 = 3;
        const OTHER_ID: u16 = 11;

        // Slots 1..=12 alternate COACT / SYNAPSE / OTHER. All start at
        // weight 0x8000 (≈ 0.5).
        for slot in 1..=12u64 {
            let ty = match slot % 3 {
                1 => COACT_ID,
                2 => SYNAPSE_ID,
                _ => OTHER_ID,
            };
            write_typed_edge(&w, slot, ty, 0x8000);
        }
        w.commit().unwrap();

        // Decay factor 0.5 (Q0.16 = 32768): COACT slots should halve, others stay.
        w.decay_all_coact(32768, 13, COACT_ID).unwrap();
        w.commit().unwrap();

        for slot in 1..=12u64 {
            let rec = w.read_edge(slot).unwrap().unwrap();
            let expected = match slot % 3 {
                1 => 0x4000, // COACT decayed (0x8000 * 0.5)
                _ => 0x8000, // others untouched
            };
            assert_eq!(
                rec.weight_u16, expected,
                "slot {slot} (type {}) — expected {expected:#x}, got {:#x}",
                rec.edge_type, rec.weight_u16,
            );
        }
    }

    #[test]
    fn decay_all_coact_skips_tombstoned() {
        // Tombstoned COACT edges must NOT be touched.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        const COACT_ID: u16 = 7;

        for slot in 1..=4u64 {
            write_typed_edge(&w, slot, COACT_ID, 0x8000);
        }
        // Tombstone slot 2.
        w.tombstone_edge(2).unwrap();
        w.commit().unwrap();

        w.decay_all_coact(32768, 5, COACT_ID).unwrap();
        w.commit().unwrap();

        // 1, 3, 4 decayed to 0x4000; 2 still 0x8000 (tombstoned, skipped).
        assert_eq!(w.read_edge(1).unwrap().unwrap().weight_u16, 0x4000);
        let rec2 = w.read_edge(2).unwrap().unwrap();
        assert_eq!(rec2.weight_u16, 0x8000);
        assert!(rec2.flags & crate::record::edge_flags::TOMBSTONED != 0);
        assert_eq!(w.read_edge(3).unwrap().unwrap().weight_u16, 0x4000);
        assert_eq!(w.read_edge(4).unwrap().unwrap().weight_u16, 0x4000);
    }

    #[test]
    fn coact_decay_wal_replay_roundtrip() {
        // T7 Step 5 crash-safety — the new `CoactDecay` payload must
        // round-trip through WAL replay. Setup: write 4 COACT edges +
        // 2 synapse edges, log a single coact decay, then replay from
        // scratch and assert the column matches the post-decay state.
        use crate::file::SubstrateFile;
        let td = tempfile::tempdir().unwrap();
        let sub = SubstrateFile::create(td.path().join("kb")).unwrap();
        let wal_path = sub.wal_path();
        let w = Writer::new(sub, SyncMode::Never).unwrap();

        const COACT_ID: u16 = 7;
        const SYNAPSE_ID: u16 = 3;
        for slot in 1..=4u64 {
            write_typed_edge(&w, slot, COACT_ID, 0x8000);
        }
        for slot in 5..=6u64 {
            write_typed_edge(&w, slot, SYNAPSE_ID, 0x8000);
        }
        w.decay_all_coact(32768, 7, COACT_ID).unwrap();
        w.commit().unwrap();
        w.wal().fsync().unwrap();
        // Drop the writer so msync fully flushes — but we explicitly want
        // replay to reapply the decay, so do NOT msync_zones here.
        drop(w);

        // Replay into a freshly-opened substrate (same dir).
        let sub2 = SubstrateFile::open(td.path().join("kb")).unwrap();
        let _ = wal_path; // silence
        let stats = crate::replay::replay_from(&sub2, 0).unwrap();
        // We logged 6 EdgeInsert + 1 CoactDecay + 1 NoOp commit = 8 records.
        assert!(
            stats.applied >= 7,
            "replay should apply our 6 inserts + decay (got {})",
            stats.applied
        );

        // Re-open through the writer to validate the column.
        let w2 = Writer::new(sub2, SyncMode::Never).unwrap();
        for slot in 1..=4u64 {
            assert_eq!(
                w2.read_edge(slot).unwrap().unwrap().weight_u16,
                0x4000,
                "COACT slot {slot} should be decayed"
            );
        }
        for slot in 5..=6u64 {
            assert_eq!(
                w2.read_edge(slot).unwrap().unwrap().weight_u16,
                0x8000,
                "SYNAPSE slot {slot} must NOT be decayed by CoactDecay"
            );
        }
    }

    /// Step 7 (RFC zorder-superposition) — `compact_community` plombe le
    /// degré RÉEL (scan d'arêtes restreint aux membres) dans les clés
    /// Hilbert/Zorder des slots relocalisés (fini le `degree = 0` dégradé).
    #[test]
    fn compact_community_uses_real_out_degree_in_keys() {
        use crate::hilbert::{hilbert_key_from_features, zorder_key_from_features};
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("cmp");
        let sub = SubstrateFile::create(&path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();

        // Communauté 1 : 6 nœuds, centralités distinctes.
        let community_id = 1u32;
        let mut deg_expected = [0u32; 7];
        let mut centr = [0u16; 7];
        for i in 1..=6u32 {
            let rec = NodeRecord {
                label_bitset: 1,
                community_id,
                centrality_cached: (i * 3000) as u16,
                ..NodeRecord::default()
            };
            centr[i as usize] = (i * 3000) as u16;
            w.write_node(i, rec).unwrap();
        }
        // Degrés : 1→2, 1→3, 2→3, 4→5 → out: node1=2, node2=1, node4=1.
        let edges: Vec<(u32, u32)> = vec![(1, 2), (1, 3), (2, 3), (4, 5)];
        for (i, &(src, dst)) in edges.iter().enumerate() {
            let rec = EdgeRecord {
                src,
                dst,
                edge_type: 1,
                ..EdgeRecord::default()
            };
            w.write_edge(i as u64 + 1, rec).unwrap();
            deg_expected[src as usize] += 1;
        }
        // Un nœud d'une autre communauté pour vérifier l'exclusion.
        {
            let rec = NodeRecord {
                label_bitset: 1,
                community_id: 2,
                ..NodeRecord::default()
            };
            w.write_node(7, rec).unwrap();
        }
        w.commit().unwrap();
        w.vider_fenetre_de_rejeu().unwrap();

        let res = w.compact_community(community_id, 8, 5, 4, 32).unwrap();
        assert!(!res.relocations.is_empty(), "compaction doit relocaliser");

        // Vérifie la colonne Zorder au NOUVEAU slot de chaque membre : clé
        // calculée avec le degré réel (plus un : les anciens slots sont zéro).
        let subg = w.substrate();
        let subg = subg.lock();
        let zf = subg.open_zone(Zone::Zorder).unwrap();
        let bytes = zf.as_slice();
        for (old, new) in &res.relocations {
            let key_at_new = u32::from_le_bytes(
                bytes[(*new as usize) * 4..(*new as usize) * 4 + 4]
                    .try_into()
                    .unwrap(),
            );
            let expected =
                zorder_key_from_features(centr[*old as usize], deg_expected[*old as usize], 4, 32);
            assert_eq!(
                key_at_new, expected,
                "zorder au slot {new} (orig {old}) doit refléter le degré réel {}",
                deg_expected[*old as usize]
            );
            let _ = hilbert_key_from_features;
        }
        // Anciens slots zéro.
        for (old, _) in &res.relocations {
            let key_at_old = u32::from_le_bytes(
                bytes[(*old as usize) * 4..(*old as usize) * 4 + 4]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(key_at_old, 0, "ancien slot {old} doit être zéro");
        }
    }
}
