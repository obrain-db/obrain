//! WAL replay — re-apply records to the mmap after a crash.
//!
//! Replay is called at boot from `SubstrateStore::open` (T4). It reads the
//! WAL from `last_checkpoint_offset` up to the first decode error (the crash
//! boundary) and re-applies each record to the zone mmaps.
//!
//! ## Idempotence
//!
//! WAL payloads carry full state, never deltas (see `wal-spec.md` §2), so
//! replaying an already-durable record is a no-op:
//!
//! * `NodeInsert` — overwrites the fixed-offset slot with identical bytes.
//! * `EdgeInsert` — same.
//! * `Checkpoint` — advisory only; used by the caller to trim the WAL.
//!
//! Records that cannot yet be applied (property pages, string heap, cognitive
//! ops) are accepted but counted as `skipped` for now — they'll be wired in
//! as T6/T7/T8 land on top of this replay primitive.

use crate::error::SubstrateResult;
use crate::file::{SubstrateFile, Zone, ZoneFile};
use crate::record::{EdgeRecord, NodeRecord, U48};
use crate::wal::{WalPayload, WalRecord};
use crate::wal_io::WalReader;
use crate::writer::{
    apply_coact_decay_to_zone, apply_energy_decay_to_zone, apply_synapse_decay_to_zone, ensure_room,
};
use tracing::{debug, warn};

/// Outcome of a replay pass.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub struct ReplayStats {
    /// Records decoded and applied cleanly.
    pub applied: u64,
    /// Records decoded but whose kind this replay version cannot apply yet
    /// (property / string / cognitive payloads — added in later tasks).
    pub skipped_unsupported: u64,
    /// Records that failed to decode (CRC mismatch, short read). The iterator
    /// stops at the first such error. Expected 0 after a clean shutdown, 1 if
    /// the tail of the WAL was torn by a crash.
    pub decode_errors: u64,
    /// Byte offset at which replay stopped. Points to the crash boundary.
    pub stopped_at: u64,
    /// Highest LSN seen during replay (or 0 if no records).
    pub last_lsn: u64,
    /// (2026-11-01, D-DUR) Transactions dont le rejeu a vu des entités en
    /// attente mais AUCUN `TxCommit` : elles étaient en vol au moment de la
    /// disparition du processus.
    pub transactions_en_vol: u64,
    /// Nœuds + arêtes de ces transactions, écartés (tombstone) en fin de rejeu.
    pub entites_en_vol_ecartees: u64,
    /// AUDIT 2026-07-25 (« maladie des coquilles », mesuree en production :
    /// 13/15 GoalNode, 25/25 AgentDirective echantillonnes sans AUCUNE
    /// propriete). Le WAL ne porte pour NodeInsert que le label_bitset —
    /// l ancre de proprietes (first_prop_off) vit dans la zone nodes en
    /// mmap, synchronisee seulement au flush. Un kill entre les deux
    /// rejoue donc des noeuds A LABELS SANS PROPRIETES : des coquilles,
    /// indistinguables d un vrai noeud au comptage. Ces deux compteurs
    /// rendent la corruption VISIBLE a l ouverture au lieu de silencieuse.
    ///
    /// Noeuds vivants dont le label est pose mais l ancre de props est
    /// nulle apres rejeu. Ambigu en theorie (un noeud sans proprietes est
    /// legal), massif en pratique uniquement apres un kill mid-flush.
    pub label_only_nodes: u64,
    /// Noeuds dont l ancre de props pointe AU-DELA de l etendue durable de
    /// la zone props : reference pendante, corruption certaine.
    pub dangling_prop_anchors: u64,
    /// (2026-09-06, fix CONSTAT-REMAP-PROPS-DETACHEMENT) Nombre de records de
    /// PERMUTATION de slots vus dans la fenêtre rejouée (`HilbertRepermute`,
    /// `HilbertRepermuteV2`, `CompactCommunity`, `CompactCommunityV2`).
    /// Consommé par `SubstrateStore::from_substrate` pour rafraîchir les owner
    /// stamps des chaînes PropsZone v2 après le rejeu : un remap committé puis
    /// un crash avant le flush suivant laisse les pages stampées à l'ancien
    /// slot — sans ce rafraîchissement, la garde d'append détacherait les
    /// chaînes à la 1ʳᵉ écriture (perte silencieuse).
    pub remaps_appliques: u32,
    /// (2026-10-06, perf #3) Nombre de slots de la zone nodes EFFECTIVEMENT
    /// balayés par l'audit post-rejeu des ancres. Rendu observable à dessein :
    /// c'est la preuve que l'audit est désormais O(fenêtre rejouée) et non
    /// O(zone entière) — un test l'assert contre la taille de la fenêtre, pas
    /// contre le nombre total de slots. Le silence (compter sans montrer le
    /// coût) serait le même poison que celui dénoncé pour les coquilles.
    pub audit_slots_scanned: u64,
}

/// Empreinte des slots de la zone nodes écrits par la fenêtre rejouée.
///
/// ── PERF #3 : borner l'audit post-rejeu aux slots touchés ──
///
/// L'audit des ancres (coquilles / pendantes) balayait TOUTE la zone nodes à
/// chaque ouverture (`for i in 0..n`), en O(N) — mesuré 169 ms pour 5 M slots,
/// INDÉPENDANT du nombre de records rejoués (3 records → même 169 ms). Or il
/// ne sert qu'aux nœuds de la fenêtre rejouée : un nœud antérieur au checkpoint
/// est déjà durable et ne peut pas porter la signature de coquille de CE crash.
/// On collecte donc ici, pendant le rejeu, les slots que le dispatcher écrit
/// réellement, sous forme d'un intervalle compact `[min, max]` — et on bascule
/// sur « toute la zone » dès qu'un record global (décroissance, repermutation)
/// a touché la zone entière, où l'intervalle ne bornerait plus.
#[derive(Copy, Clone)]
struct SlotsTouches {
    min: u32,
    max: u32,
    /// Un record a touché la zone ENTIÈRE : l'intervalle compact ne suffit plus.
    toute_la_zone: bool,
}

impl SlotsTouches {
    /// Aucun slot touché (`min > max`) — état initial avant le rejeu.
    const AUCUN: Self = Self {
        min: u32::MAX,
        max: 0,
        toute_la_zone: false,
    };

    fn noter(&mut self, slot: u32) {
        if self.toute_la_zone {
            return; // déjà couvert par le balayage complet
        }
        self.min = self.min.min(slot);
        self.max = self.max.max(slot);
    }

    fn noter_tout(&mut self) {
        self.toute_la_zone = true;
    }

    /// Plage INCLUSIVE `[début, fin]` à auditer. `None` = rien à auditer
    /// (fenêtre sans aucun slot de nœud écrit). L'appelant borne à la longueur
    /// réelle de la zone.
    fn plage(&self) -> Option<(u32, u32)> {
        if self.toute_la_zone {
            Some((0, u32::MAX))
        } else if self.min <= self.max {
            Some((self.min, self.max))
        } else {
            None
        }
    }
}

/// Replay the WAL of `substrate` starting at `from_offset`.
///
/// The function opens the substrate's WAL file, decodes records in order,
/// and applies each to the corresponding zone mmap. It stops at the first
/// decode error.
pub fn replay_from(substrate: &SubstrateFile, from_offset: u64) -> SubstrateResult<ReplayStats> {
    replay_from_avec(substrate, from_offset, true)
}

/// Rejeu d'un LECTEUR RO : identique à [`replay_from`], sauf qu'il n'écarte PAS
/// les transactions en vol.
///
/// Écarter une transaction non scellée, c'est poser des tombstones dans les
/// zones. C'est la bonne reprise quand son propriétaire est mort ; ce ne l'est
/// pas quand un écrivain partagé VIVANT l'a ouverte et n'a pas encore posé son
/// `TxCommit` : le lecteur (verrou `LOCK_SH`, comme l'écrivain) tuerait alors un
/// nœud légitime, et personne ne le republierait. Un lecteur ne sait pas si
/// l'écrivain est vivant ; il ne doit donc rien écrire dans les zones qu'il
/// partage.
pub fn replay_from_lecteur(
    substrate: &SubstrateFile,
    from_offset: u64,
) -> SubstrateResult<ReplayStats> {
    replay_from_avec(substrate, from_offset, false)
}

fn replay_from_avec(
    substrate: &SubstrateFile,
    from_offset: u64,
    ecarter_en_vol: bool,
) -> SubstrateResult<ReplayStats> {
    let wal_path = substrate.wal_path();
    let reader = WalReader::open(&wal_path)?;
    let mut stats = ReplayStats {
        stopped_at: from_offset,
        ..Default::default()
    };
    if reader.is_empty() {
        return Ok(stats);
    }

    // ── UN OPCODE INCONNU REFUSE LE REJEU AVANT D'APPLIQUER (2026-11-07) ───
    // Une frame entière à CRC valide dont le `kind` est hors table vient d'un
    // binaire PLUS RÉCENT ; tout ce qui la suit nous est illisible. Jusqu'ici
    // elle sortait en `WalBadFrame`, confondue avec une frontière de crash :
    // la base s'ouvrait sur un préfixe, les transactions en vol étaient
    // tombstonées sur un rejeu tronqué, et le checkpoint suivant TRONQUAIT le
    // WAL — perte définitive des frames postérieures. Le balayage se fait
    // AVANT d'ouvrir la moindre zone pour que le refus ne laisse rien
    // derrière lui : ni frame appliquée, ni tombstone. (Le bras `Err` de la
    // boucle porte le même refus pour le cas que le balayage ne tranche pas :
    // un opcode inconnu situé APRÈS une anomalie.)
    if let Some(e) = reader.premier_opcode_inconnu(from_offset) {
        tracing::error!(
            error = %e,
            from_offset,
            "rejeu WAL REFUSÉ : opcode inconnu — ce WAL a été écrit par une version \
             plus récente. Rien n'a été appliqué ; rouvrir avec cette version, ou \
             checkpointer avec elle avant de rétrograder."
        );
        return Err(e);
    }

    // Lazily open zones we need.
    let mut nodes: Option<ZoneFile> = None;
    let mut edges: Option<ZoneFile> = None;
    let mut engram_members: Option<ZoneFile> = None;
    let mut engram_bitset: Option<ZoneFile> = None;
    let mut hilbert: Option<ZoneFile> = None;
    let mut community: Option<ZoneFile> = None;
    let mut zorder: Option<ZoneFile> = None;
    let mut simplex = SimplexReplayZones::default();
    // ── PERF #3 ── empreinte des slots de nœuds touchés par le rejeu, pour
    // borner l'audit post-rejeu des ancres à la fenêtre rejouée au lieu de la
    // zone entière.
    let mut touches = SlotsTouches::AUCUN;
    // (2026-11-01, D-DUR) Entités déclarées en attente par transaction, tant
    // qu'aucun `TxCommit` ne les a publiées. Ce qui reste à la fin est en vol.
    let mut en_vol: std::collections::HashMap<u64, (Vec<u32>, Vec<u64>)> =
        std::collections::HashMap::new();

    // On tient l'itérateur (au lieu d'un `for`) pour pouvoir lire, APRÈS la
    // boucle, le drapeau « queue incomplète » — l'information que l'itérateur
    // ne rend plus comme une erreur (une écriture en vol n'est pas une
    // corruption) mais qui reste due à l'appelant.
    let mut iter = reader.iter_from(from_offset);
    for item in iter.by_ref() {
        match item {
            Ok((rec, off, len)) => {
                stats.last_lsn = stats.last_lsn.max(rec.lsn);
                match &rec.payload {
                    WalPayload::TxNoeudEnAttente { tx, node_id } => {
                        en_vol.entry(*tx).or_default().0.push(*node_id);
                    }
                    WalPayload::TxAreteEnAttente { tx, edge_id } => {
                        en_vol.entry(*tx).or_default().1.push(*edge_id);
                    }
                    WalPayload::TxCommit { tx } => {
                        en_vol.remove(tx);
                    }
                    // Une entité SUPPRIMÉE n'est plus en vol — et son slot
                    // peut être réutilisé par une création ultérieure (hors
                    // transaction, donc sans record d'attente). Sans ce
                    // retrait, le rejeu d'un ROLLBACK (NodeDelete) suivi d'une
                    // réutilisation du slot tombstonerait un nœud légitime.
                    WalPayload::NodeDelete { node_id } => {
                        for (n, _) in en_vol.values_mut() {
                            n.retain(|id| id != node_id);
                        }
                    }
                    WalPayload::EdgeDelete { edge_id } => {
                        for (_, e) in en_vol.values_mut() {
                            e.retain(|id| id != edge_id);
                        }
                    }
                    _ => {}
                }
                // (2026-09-06, fix CONSTAT-REMAP-PROPS-DETACHEMENT) Compte les
                // records de permutation de slots de la fenêtre : le store
                // rafraîchit ensuite les owner stamps des chaînes PropsZone v2
                // (fenêtre power-loss entre le commit d'un remap et le flush).
                if matches!(
                    rec.payload,
                    WalPayload::HilbertRepermute { .. }
                        | WalPayload::HilbertRepermuteV2 { .. }
                        | WalPayload::CompactCommunity { .. }
                        | WalPayload::CompactCommunityV2 { .. }
                ) {
                    stats.remaps_appliques += 1;
                }
                match apply(
                    substrate,
                    &rec,
                    &mut nodes,
                    &mut edges,
                    &mut engram_members,
                    &mut engram_bitset,
                    &mut hilbert,
                    &mut community,
                    &mut zorder,
                    &mut simplex,
                    &mut touches,
                )? {
                    Applied::Yes => stats.applied += 1,
                    Applied::Skipped => stats.skipped_unsupported += 1,
                }
                stats.stopped_at = off + len as u64;
            }
            Err(e) => {
                // ── DEUX CAUSES, UNE SEULE SORTIE — et c'est le piege ──
                //
                // Un arret ici signifie soit une frontiere de crash (cas
                // NORMAL : le WAL a ete tronque par une panne, tout ce qui
                // precede est bon), soit un OPCODE INCONNU.
                //
                // Le second cas arrive apres un DOWNGRADE de binaire : les
                // opcodes 0x54 / 0x55 (repermutations V2) n'existent pas
                // dans un binaire anterieur, qui s'arreterait donc net —
                // en silence, sur une base potentiellement a moitie
                // remappee. D'ou le niveau ERROR et la mention explicite
                // de la regle : voir `crate::slot_remap`, section
                // « Retrograder un binaire ».
                // Diagnostic du downgrade. `?` et non `unwrap_or_default` :
                // une zone d'epoch illisible est elle-meme un refus (revue
                // #3), et surtout pas une base a considerer comme vierge.
                //
                // ── UN TROU N'EST PAS UNE FRONTIÈRE DE CRASH (2026-10-06) ──
                // Le message ci-dessous dit « crash boundary » : c'est vrai
                // d'une queue déchirée, FAUX d'un trou de zéros suivi de
                // données. Confondre les deux ferait passer une perte de
                // contenu pour une panne ordinaire — et le rejeu s'arrêterait
                // sans que personne ne sache qu'il reste des frames APRÈS le
                // défaut. Le cas est donc nommé séparément, avant tout
                // diagnostic de downgrade (qui, lui, n'a rien à en dire).
                //
                // ── UN OPCODE INCONNU N'EST PAS UNE FRONTIÈRE DE CRASH (2026-11-07)
                // Le pré-balayage en tête de fonction a déjà refusé le cas
                // ordinaire ; on n'arrive ici qu'avec un opcode inconnu situé
                // APRÈS une anomalie (trou sauté, par exemple). Le verdict est
                // le même : REFUS propagé, pas de `break` vers un `Ok(stats)`
                // tronqué, pas de tombstones sur un rejeu partiel.
                if let crate::SubstrateError::WalOpcodeInconnu { kind, offset } = &e {
                    tracing::error!(
                        kind = format_args!("{kind:#04X}"),
                        offset,
                        stopped_at = stats.stopped_at,
                        "rejeu WAL REFUSÉ sur un opcode inconnu : ce WAL a été écrit par une \
                         version plus récente ; rouvrir avec cette version, ou checkpointer \
                         avec elle avant de rétrograder."
                    );
                    return Err(e);
                }
                if let crate::SubstrateError::WalTrou { offset, longueur } = &e {
                    tracing::error!(
                        offset,
                        longueur,
                        stopped_at = stats.stopped_at,
                        "rejeu WAL arrêté sur un TROU de zéros : ce n'est PAS une frontière \
                         de crash — le fichier continue APRÈS le trou, et ces frames ne sont \
                         PAS appliquées. Resynchronisez, ne tronquez pas."
                    );
                    stats.decode_errors += 1;
                    break;
                }
                let epoch = crate::slot_remap::remap_epoch_state(substrate)?;
                if epoch.committed != 0 || epoch.pending != 0 {
                    tracing::error!(
                        error = %e,
                        stopped_at = stats.stopped_at,
                        epoch_committed = epoch.committed,
                        epoch_pending = epoch.pending,
                        "rejeu WAL interrompu sur une base DEJA REPERMUTEE : si ce binaire \
                         est anterieur au chantier SlotRemappable, il ne sait pas lire les \
                         opcodes 0x54/0x55 et vient de s'arreter sur une base qu'il ne \
                         comprend pas. REGLE : checkpoint force AVANT tout downgrade."
                    );
                } else {
                    warn!(
                        error = %e,
                        stopped_at = stats.stopped_at,
                        "wal replay halted at crash boundary"
                    );
                }
                stats.decode_errors += 1;
                break;
            }
        }
    }
    // ── LES TRANSACTIONS EN VOL SONT ÉCARTÉES, PAS PUBLIÉES (2026-11-01) ────
    // Une transaction qui a déclaré des entités en attente sans jamais être
    // scellée par un `TxCommit` était ouverte quand le processus a disparu
    // (ou a été refusée/annulée : ses tombstones sont alors déjà dans le
    // journal, et les re-poser est idempotent). Sans ce bloc, ses nœuds et
    // arêtes — physiquement dans les zones — revenaient publiés à la
    // réouverture : 50/50 mesurés sur `durabilite_commit_kill9`.
    en_vol.retain(|_, (n, e)| !n.is_empty() || !e.is_empty());
    if ecarter_en_vol && !en_vol.is_empty() {
        for (tx, (noeuds, aretes)) in &en_vol {
            stats.transactions_en_vol += 1;
            for edge_id in aretes {
                let rec = WalRecord {
                    lsn: 0,
                    timestamp: 0,
                    flags: 0,
                    payload: WalPayload::EdgeDelete { edge_id: *edge_id },
                };
                apply(
                    substrate,
                    &rec,
                    &mut nodes,
                    &mut edges,
                    &mut engram_members,
                    &mut engram_bitset,
                    &mut hilbert,
                    &mut community,
                    &mut zorder,
                    &mut simplex,
                    &mut touches,
                )?;
                stats.entites_en_vol_ecartees += 1;
            }
            for node_id in noeuds {
                let rec = WalRecord {
                    lsn: 0,
                    timestamp: 0,
                    flags: 0,
                    payload: WalPayload::NodeDelete { node_id: *node_id },
                };
                apply(
                    substrate,
                    &rec,
                    &mut nodes,
                    &mut edges,
                    &mut engram_members,
                    &mut engram_bitset,
                    &mut hilbert,
                    &mut community,
                    &mut zorder,
                    &mut simplex,
                    &mut touches,
                )?;
                stats.entites_en_vol_ecartees += 1;
            }
            tracing::warn!(
                tx,
                noeuds = noeuds.len(),
                aretes = aretes.len(),
                "rejeu WAL : transaction EN VOL (jamais scellée) — ses entités sont écartées, pas publiées"
            );
        }
    }

    // ── LA FRONTIÈRE DE CRASH SE DIT TOUJOURS (2026-09-17) ──────────────────
    // L'itérateur ne rend plus une queue incomplète comme une erreur — c'est une
    // écriture en vol, pas une corruption, et le crier faisait échouer un test
    // qui avait raison. Mais `decode_errors` documente « 1 si la queue du WAL a
    // été déchirée par un crash » et `stopped_at` « pointe sur la frontière de
    // crash » : rendre la queue SILENCIEUSE aurait transformé une panne en fin de
    // fichier ordinaire. Le fait est donc compté ici, exactement comme avant —
    // seul le canal a changé (un drapeau au lieu d'une erreur de décodage).
    if iter.queue_incomplete() {
        stats.decode_errors += 1;
        warn!(
            stopped_at = stats.stopped_at,
            applied = stats.applied,
            "rejeu WAL arrêté sur une queue INCOMPLÈTE (frontière de crash : écriture \
             en vol ou arrêt brutal) — tout ce qui précède est appliqué"
        );
    }

    // Msync + fsync whatever zones we touched so the replay state is durable.
    if let Some(zf) = nodes.as_ref() {
        zf.msync()?;
        zf.fsync()?;
    }
    if let Some(zf) = edges.as_ref() {
        zf.msync()?;
        zf.fsync()?;
    }
    if let Some(zf) = engram_members.as_ref() {
        zf.msync()?;
        zf.fsync()?;
    }
    if let Some(zf) = engram_bitset.as_ref() {
        zf.msync()?;
        zf.fsync()?;
    }
    if let Some(zf) = hilbert.as_ref() {
        zf.msync()?;
        zf.fsync()?;
    }
    if let Some(zf) = community.as_ref() {
        zf.msync()?;
        zf.fsync()?;
    }
    for zf in [
        simplex.dir.as_ref(),
        simplex.heap.as_ref(),
        simplex.incidence.as_ref(),
        simplex.postings.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        zf.msync()?;
        zf.fsync()?;
    }

    // ── AUDIT 2026-07-25 : bilan de sante des ancres apres rejeu ──
    // Scan lineaire de la zone nodes : compte les coquilles (labels sans
    // ancre de props) et les ancres pendantes (pointant hors de l etendue
    // durable de la zone props). Aucune mutation — v1 observabilite pure :
    // le silence etait le vrai poison (l etage teleologique de l agent a
    // ete detruit deux fois sans qu aucun signal n existe nulle part).
    {
        let nodes_zf = match nodes {
            Some(zf) => Some(zf),
            None => substrate.open_zone(Zone::Nodes).ok(),
        };
        if let Some(zf) = nodes_zf {
            let regime = RegimeAncres::detecter(substrate);
            let buf = zf.as_slice();
            let n = buf.len() / NodeRecord::SIZE;
            // ── PERF #3 : bornage de l'audit aux slots touchés ──
            // L'ancien `for i in 0..n` balayait la zone entière à chaque
            // ouverture (O(N), 169 ms pour 5 M slots même avec 3 records
            // rejoués). Ici on ne balaie que [min_touché, max_touché] — ou
            // rien si la fenêtre rejouée n'a écrit aucun nœud, ou toute la
            // zone si un record global (décroissance / repermutation) l'a
            // touchée entière. Un nœud hors de la fenêtre est antérieur au
            // checkpoint : durable, il ne peut pas porter la coquille de CE
            // crash — le sauter est correct, pas une approximation.
            let (debut, fin_excl) = match touches.plage() {
                None => (n, n),
                Some((d, f)) => {
                    let debut = (d as usize).min(n);
                    let fin_excl = ((f as u64) + 1).min(n as u64) as usize;
                    (debut, fin_excl)
                }
            };
            stats.audit_slots_scanned = fin_excl.saturating_sub(debut) as u64;
            for i in debut..fin_excl {
                let rec: &NodeRecord =
                    bytemuck::from_bytes(&buf[i * NodeRecord::SIZE..(i + 1) * NodeRecord::SIZE]);
                // ── LE SLOT 0 N'EST PAS UN NŒUD (2026-09-16) ─────────────────
                // Le prédicat est celui du substrat, PAS un test d'étiquette.
                //
                // Défaut mesuré : sur TOUT store v3 — y compris un store VIDE,
                // `count(n) = 0` — cet audit annonçait `label_only = 1` et
                // « properties written after the last flush were lost », alors
                // qu'aucune propriété n'était perdue (la relecture rendait
                // chacune). Cause, écrite dans `record.rs` : une zone v3 grave
                // `NODES_ZONE_V3_MAGIC` dans le `label_bitset` de son SLOT 0 —
                // c'est son marqueur de stride — et laisse ses flags à zéro.
                // Ce record « n'existe pour aucun chemin de lecture »
                // (`is_live()` faux), mais `label_bitset != 0` suffisait à
                // l'ancien test pour le prendre pour une coquille.
                //
                // Conséquence : une garde qui criait sur 100 % des stores noie
                // le seul signal qu'elle existe pour porter. Le doc du compteur
                // ci-dessus dit d'ailleurs « Nœuds VIVANTS » — le code ne le
                // faisait pas.
                if !rec.is_live() {
                    continue; // slot vide, sentinelle de zone, ou tombstone
                }
                let anchor: u64 = rec.first_prop_off.to_u64();
                if anchor == 0 {
                    stats.label_only_nodes += 1;
                } else if regime.ancre_pendante(anchor) {
                    stats.dangling_prop_anchors += 1;
                }
            }
            if stats.label_only_nodes > 0 || stats.dangling_prop_anchors > 0 {
                tracing::warn!(
                    label_only = stats.label_only_nodes,
                    dangling = stats.dangling_prop_anchors,
                    regime = regime.nom(),
                    "post-replay anchor audit: nodes with labels but no reachable properties                      (husk signature — properties written after the last flush were lost)"
                );
            }
        }
    }

    debug!(?stats, "wal replay complete");
    Ok(stats)
}

/// Comment lire `NodeRecord::first_prop_off` pour l audit d ancres.
///
/// ── (2026-08-19) L AUDIT CRIAIT AU LOUP SOUS PropsZone v2 ──
///
/// L audit du 25/07 comparait l ancre à la LONGUEUR du fichier
/// `substrate.props`. C était juste tant que l ancre était un décalage en
/// octets DANS ce fichier. Sous PropsZone v2 elle est un IDENTIFIANT DE PAGE
/// dans `substrate.props.v2` (`encode_page_id` ; page 0 = sentinelle « pas de
/// propriété ») : plus la même unité, plus le même fichier. Tant que
/// `substrate.props` restait vide, la garde `props_len > 0` rendait
/// `dangling = 0` PAR ACCIDENT ; la première écriture qui recrée ce fichier
/// faisait basculer le compteur.
///
/// MESURE (19/08, validation sur base réelle) : `dangling = 49 850` sur une
/// base saine, dont l intégrité venait d être vérifiée par ailleurs — 4,47 M
/// de lectures comparées, zéro valeur perdue. Faux positif intégral. À
/// ~1 528 bases à migrer, ce bruit noierait le seul signal que l audit existe
/// pour porter : la « signature de coquille ».
///
/// CE QUE « PENDANT » VEUT DIRE SOUS v2 — 89f5ca5e a mesuré les formes de
/// tête de chaîne invalide sur la base `messages` :
///   · index au-delà de la zone            1 501 040 refus ;
///   · page en magic 0x0, jamais écrite       112 280 refus.
/// Les deux se ramènent à UN test : l index tombe hors de `1..frontière`, où
/// `frontière` est le premier index jamais alloué. La région au-delà de la
/// frontière EST la région jamais écrite — la frontière est franche par
/// construction (invariant documenté sur `compute_next_page_idx` :
/// allocation monotone, pas de trou) — et elle s obtient en O(log N) lectures
/// par recherche dichotomique.
///
/// POURQUOI PAS UNE SONDE DE MAGIE PAR NŒUD : l audit tourne à CHAQUE
/// ouverture, après le rejeu. Lire les 4 octets de magie de la page de chaque
/// nœud ancré coûterait une page de 4 Kio par nœud — 1,8 Go sur `messages`
/// (454 073 pages, 1:1 avec les nœuds), et l ordre de la Megalaw
/// (props.v2 de 103 Go) — pour détecter ce que la frontière couvre déjà.
///
/// RESTE NON COUVERT, ASSUMÉ : une page allouée mais jamais écrite SOUS la
/// frontière (trou laissé par un kill entre l allocation et l écriture), et
/// les deux autres formes de 89f5ca5e — page d un autre genre, page d un
/// AUTRE propriétaire. Ces trois-là désignent une page RÉELLE : ce n est plus
/// une référence pendante mais une tête qui ment, et les débusquer demande
/// exactement la lecture par nœud qu on vient d écarter.
enum RegimeAncres {
    /// `substrate.props.v2` présent : l ancre est un identifiant de page.
    /// `frontiere` = premier index jamais alloué (pages vivantes = `1..frontiere`).
    PagesV2 { frontiere: u64 },
    /// Base héritée : l ancre est un décalage en octets dans `substrate.props`.
    /// Critère d origine conservé tel quel — c est le seul qui ait un sens
    /// là-bas, et ces bases restent la majorité tant que la migration n a pas
    /// tourné.
    OctetsHerites { longueur: u64 },
}

impl RegimeAncres {
    fn detecter(substrate: &SubstrateFile) -> Self {
        let chemin_v2 = substrate.path().join(crate::props_zone::PROPS_V2_FILENAME);
        if chemin_v2.exists()
            && let Ok(zf) = substrate.open_named_zone(crate::props_zone::PROPS_V2_FILENAME)
            && !zf.is_empty()
        {
            return Self::PagesV2 {
                frontiere: crate::props_zone::PropsZone::compute_next_page_idx(&zf) as u64,
            };
        }
        // `substrate.props` n est ouvert QUE dans le régime hérité : `open_zone`
        // est un `open_or_create`, et le recréer sous v2 ressusciterait
        // précisément le fichier dont la longueur nous a menti.
        Self::OctetsHerites {
            longueur: substrate
                .open_zone(Zone::Props)
                .map(|p| p.as_slice().len() as u64)
                .unwrap_or(0),
        }
    }

    /// `ancre` est non nulle (le zéro est compté en coquille par l appelant).
    fn ancre_pendante(&self, ancre: u64) -> bool {
        match *self {
            Self::PagesV2 { frontiere } => {
                // (feat slots512-wiring) Sous env slots, une tête peut être un
                // SLOT packé (bit 47) : valide si carrier < frontière ET unité
                // 1..=7 — sinon, tête legacy : index < frontière (règle
                // d'origine, 19/08). Une tête slot est posée par le store
                // APRÈS l'écriture du slot → pointe toujours une unité allouée.
                if ancre & crate::unite::UNITE_SLOT_FLAG != 0 {
                    let corps = ancre & !crate::unite::UNITE_SLOT_FLAG;
                    let carrier = corps >> 4;
                    let unite = (corps & 0xF) as u8;
                    carrier >= frontiere
                        || unite == 0
                        || unite as usize > crate::unite::SLOTS_UTILISABLES_PAR_CARRIER
                } else {
                    ancre >= frontiere
                }
            }
            Self::OctetsHerites { longueur } => longueur > 0 && ancre >= longueur,
        }
    }

    fn nom(&self) -> &'static str {
        match self {
            Self::PagesV2 { .. } => "pages_v2",
            Self::OctetsHerites { .. } => "octets_herites",
        }
    }
}

enum Applied {
    Yes,
    Skipped,
}

/// Handles paresseux des quatre zones simpliciales (T4). Ouvertes au
/// premier record 0x72/0x73 rencontré, msync'ées en fin de rejeu.
#[derive(Default)]
struct SimplexReplayZones {
    dir: Option<ZoneFile>,
    heap: Option<ZoneFile>,
    incidence: Option<ZoneFile>,
    postings: Option<ZoneFile>,
}

impl SimplexReplayZones {
    fn open(&mut self, substrate: &SubstrateFile) -> SubstrateResult<()> {
        if self.dir.is_none() {
            self.dir = Some(substrate.open_zone(Zone::SimplexDir)?);
            self.heap =
                Some(substrate.open_named_zone(crate::simplex_zone::SIMPLEX_HEAP_FILENAME)?);
            self.incidence = Some(substrate.open_zone(Zone::SimplexIncidence)?);
            self.postings = Some(
                substrate.open_named_zone(crate::simplex_incidence::SIMPLEX_POSTINGS_FILENAME)?,
            );
        }
        Ok(())
    }
}

// Private WAL-replay dispatcher: the zone-file handles are naturally
// individual parameters; bundling them into a struct would not clarify.
#[allow(clippy::too_many_arguments)]
fn apply(
    substrate: &SubstrateFile,
    rec: &WalRecord,
    nodes: &mut Option<ZoneFile>,
    edges: &mut Option<ZoneFile>,
    engram_members_zone: &mut Option<ZoneFile>,
    engram_bitset_zone: &mut Option<ZoneFile>,
    hilbert: &mut Option<ZoneFile>,
    community: &mut Option<ZoneFile>,
    zorder: &mut Option<ZoneFile>,
    simplex: &mut SimplexReplayZones,
    touches: &mut SlotsTouches,
) -> SubstrateResult<Applied> {
    match &rec.payload {
        WalPayload::NodeInsert {
            node_id,
            label_bitset,
        } => {
            touches.noter(*node_id);
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            let offset = (*node_id as usize) * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, 1 << 20)?;
            // The WAL records the *label_bitset* specifically for NodeInsert;
            // the rest of the NodeRecord is filled in later by subsequent
            // records (NodeUpdate, EnergyReinforce, etc.). Idempotent: writing
            // the same label_bitset twice is a no-op.
            let existing: &NodeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
            let mut updated = *existing;
            updated.label_bitset = *label_bitset;
            // (jalon 1) un NodeInsert rejoue est un record reellement ecrit.
            updated.flags |= crate::record::node_flags::ALLOCATED;
            // (jalon 2) trame ANCIENNE, sans tenant : le tenant existant est
            // PRESERVE (zone vierge -> 0 = public, coherent avec la
            // migration en bloc declaree). Les writers v3 emettent
            // NodeInsertV3.
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::NodeInsertV3 {
            node_id,
            label_bitset,
            tenant_id,
        } => {
            touches.noter(*node_id);
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            let offset = (*node_id as usize) * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, 1 << 20)?;
            let existing: &NodeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
            let mut updated = *existing;
            updated.label_bitset = *label_bitset;
            updated.flags |= crate::record::node_flags::ALLOCATED;
            // (jalon 2) la trame V3 restaure l'appartenance.
            updated.tenant_id = *tenant_id;
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::NodeUpdate {
            node_id,
            label_bitset,
            energy,
            scar_util_affinity,
            centrality_cached,
            flags,
            community_id,
        } => {
            touches.noter(*node_id);
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            let offset = (*node_id as usize) * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, 1 << 20)?;
            let existing: &NodeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
            let mut updated = *existing;
            updated.label_bitset = *label_bitset;
            updated.energy = *energy;
            updated.scar_util_affinity = *scar_util_affinity;
            updated.centrality_cached = *centrality_cached;
            // (jalon 1) OR force : un WAL v1 residuel ne porte pas le bit.
            updated.flags = *flags | crate::record::node_flags::ALLOCATED;
            updated.community_id = *community_id;
            // (jalon 2) trame ANCIENNE : tenant existant preserve — voir
            // NodeInsert.
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::NodeUpdateV3 {
            node_id,
            label_bitset,
            energy,
            scar_util_affinity,
            centrality_cached,
            flags,
            community_id,
            tenant_id,
        } => {
            touches.noter(*node_id);
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            let offset = (*node_id as usize) * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, 1 << 20)?;
            let existing: &NodeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
            let mut updated = *existing;
            updated.label_bitset = *label_bitset;
            updated.energy = *energy;
            updated.scar_util_affinity = *scar_util_affinity;
            updated.centrality_cached = *centrality_cached;
            updated.flags = *flags | crate::record::node_flags::ALLOCATED;
            updated.community_id = *community_id;
            // (jalon 2) la trame V3 restaure l'appartenance.
            updated.tenant_id = *tenant_id;
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::EdgeInsert {
            edge_id,
            src,
            dst,
            edge_type,
            weight_u16,
        } => {
            if edges.is_none() {
                *edges = Some(substrate.open_zone(Zone::Edges)?);
            }
            let zf = edges.as_mut().unwrap();
            let offset = (*edge_id as usize) * EdgeRecord::SIZE;
            ensure_room(zf, offset + EdgeRecord::SIZE, 1 << 20)?;
            let existing: &EdgeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + EdgeRecord::SIZE]);
            let mut updated = *existing;
            updated.src = *src;
            updated.dst = *dst;
            updated.edge_type = *edge_type;
            updated.weight_u16 = *weight_u16;
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::NodeDelete { node_id } => {
            touches.noter(*node_id);
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            let offset = (*node_id as usize) * NodeRecord::SIZE;
            if offset + NodeRecord::SIZE > zf.as_slice().len() {
                // Slot never materialised — tombstone is implicit.
                return Ok(Applied::Yes);
            }
            let existing: &NodeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
            let mut updated = *existing;
            updated.set_tombstoned();
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::EdgeUpdate {
            edge_id,
            weight_u16,
            ricci_u8,
            flags,
            engram_tag,
        } => {
            if edges.is_none() {
                *edges = Some(substrate.open_zone(Zone::Edges)?);
            }
            let zf = edges.as_mut().unwrap();
            let offset = (*edge_id as usize) * EdgeRecord::SIZE;
            if offset + EdgeRecord::SIZE > zf.as_slice().len() {
                // Slot never materialised; skip silently.
                return Ok(Applied::Yes);
            }
            let existing: &EdgeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + EdgeRecord::SIZE]);
            let mut updated = *existing;
            updated.weight_u16 = *weight_u16;
            updated.ricci_u8 = *ricci_u8;
            updated.flags = *flags;
            updated.engram_tag = *engram_tag;
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::EdgeDelete { edge_id } => {
            if edges.is_none() {
                *edges = Some(substrate.open_zone(Zone::Edges)?);
            }
            let zf = edges.as_mut().unwrap();
            let offset = (*edge_id as usize) * EdgeRecord::SIZE;
            if offset + EdgeRecord::SIZE > zf.as_slice().len() {
                return Ok(Applied::Yes);
            }
            let existing: &EdgeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + EdgeRecord::SIZE]);
            let mut updated = *existing;
            updated.flags |= crate::record::edge_flags::TOMBSTONED;
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        // ---- Cognitive column payloads (T6) --------------------------------
        WalPayload::EnergyReinforce {
            node_id,
            new_energy,
        } => {
            touches.noter(*node_id);
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            let offset = (*node_id as usize) * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, 1 << 20)?;
            let existing: &NodeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
            let mut updated = *existing;
            updated.energy = *new_energy;
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::EnergyDecay { factor_q16 } => {
            // Touche chaque slot vivant de la zone : l'intervalle compact ne
            // borne plus, on retombe sur un balayage complet (correct, car la
            // décroissance réécrit l'énergie de tous les nœuds).
            touches.noter_tout();
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            // Decay every live slot in the zone. Use u32::MAX so the
            // function walks to end-of-zone — unknown high-water at replay
            // time, but beyond zone length the loop breaks. Tombstoned
            // slots are skipped internally.
            let slots = (zf.len() as usize / NodeRecord::SIZE) as u32;
            apply_energy_decay_to_zone(zf, *factor_q16, slots);
            Ok(Applied::Yes)
        }

        WalPayload::SynapseReinforce {
            edge_id,
            new_weight,
        } => {
            if edges.is_none() {
                *edges = Some(substrate.open_zone(Zone::Edges)?);
            }
            let zf = edges.as_mut().unwrap();
            let offset = (*edge_id as usize) * EdgeRecord::SIZE;
            ensure_room(zf, offset + EdgeRecord::SIZE, 1 << 20)?;
            let existing: &EdgeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + EdgeRecord::SIZE]);
            let mut updated = *existing;
            updated.weight_u16 = *new_weight;
            zf.as_slice_mut()[offset..offset + EdgeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::SynapseDecay { factor_q16 } => {
            if edges.is_none() {
                *edges = Some(substrate.open_zone(Zone::Edges)?);
            }
            let zf = edges.as_mut().unwrap();
            // Walk every live slot to end-of-zone. Unknown high-water at
            // replay time — the loop breaks when offset+stride exceeds
            // zone length. Tombstoned slots are skipped internally.
            let slots = (zf.len() as usize / EdgeRecord::SIZE) as u64;
            apply_synapse_decay_to_zone(zf, *factor_q16, slots);
            Ok(Applied::Yes)
        }

        WalPayload::CoactDecay {
            factor_q16,
            coact_type_id,
        } => {
            if edges.is_none() {
                *edges = Some(substrate.open_zone(Zone::Edges)?);
            }
            let zf = edges.as_mut().unwrap();
            let slots = (zf.len() as usize / EdgeRecord::SIZE) as u64;
            apply_coact_decay_to_zone(zf, *factor_q16, slots, *coact_type_id);
            Ok(Applied::Yes)
        }

        WalPayload::ScarUtilAffinitySet { node_id, packed } => {
            touches.noter(*node_id);
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            let offset = (*node_id as usize) * NodeRecord::SIZE;
            ensure_room(zf, offset + NodeRecord::SIZE, 1 << 20)?;
            let existing: &NodeRecord =
                bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
            let mut updated = *existing;
            updated.scar_util_affinity = *packed;
            zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                .copy_from_slice(bytemuck::bytes_of(&updated));
            Ok(Applied::Yes)
        }

        WalPayload::CentralityUpdate { updates } => {
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            for (node_id, centrality) in updates {
                touches.noter(*node_id);
                let offset = (*node_id as usize) * NodeRecord::SIZE;
                ensure_room(zf, offset + NodeRecord::SIZE, 1 << 20)?;
                let existing: &NodeRecord =
                    bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
                let mut updated = *existing;
                updated.centrality_cached = *centrality;
                updated.flags &= !crate::record::node_flags::CENTRALITY_STALE;
                zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                    .copy_from_slice(bytemuck::bytes_of(&updated));
            }
            Ok(Applied::Yes)
        }

        // Community drive — T10. LDleiden persists node→community assignments
        // as a batch of `(node_id, community_id)` tuples. Replay writes the
        // column idempotently: re-applying the same batch overwrites the slot
        // with the same bytes.
        WalPayload::CommunityAssign { updates } => {
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            for (node_id, community_id) in updates {
                touches.noter(*node_id);
                let offset = (*node_id as usize) * NodeRecord::SIZE;
                ensure_room(zf, offset + NodeRecord::SIZE, 1 << 20)?;
                let existing: &NodeRecord =
                    bytemuck::from_bytes(&zf.as_slice()[offset..offset + NodeRecord::SIZE]);
                let mut updated = *existing;
                updated.community_id = *community_id;
                zf.as_slice_mut()[offset..offset + NodeRecord::SIZE]
                    .copy_from_slice(bytemuck::bytes_of(&updated));
            }
            Ok(Applied::Yes)
        }

        // Zones simpliciales (T4) — etat absolu, gabarit 0x70/0x71.
        WalPayload::SimplexSet {
            simplex_id,
            kind,
            flags,
            type_id,
            members,
            props,
        } => {
            simplex.open(substrate)?;
            let applied = crate::simplex_zone::apply_set_raw(
                simplex.dir.as_mut().unwrap(),
                simplex.heap.as_mut().unwrap(),
                *simplex_id,
                *kind,
                *flags,
                *type_id,
                members,
                props,
            )?;
            if applied == crate::simplex_zone::AppliedSimplex::Materialized {
                for &m in members {
                    crate::simplex_incidence::SimplexIncidenceZone::append(
                        simplex.incidence.as_mut().unwrap(),
                        simplex.postings.as_mut().unwrap(),
                        m,
                        *simplex_id,
                    )?;
                }
            } else {
                // Record DÉJÀ présent (Unchanged/Rewritten) : c'est l'état d'un
                // kill -9 pendant la pose des postings (le record du répertoire
                // est écrit avant eux). Un record vivant doit avoir son
                // incidence complète : on comble les manques, sans doublon.
                // Un tombstone n'est pas reposté (ses postings peuvent avoir été
                // réclamés par compact_simplexes).
                let vivant = crate::simplex_zone::SimplexDirZone::read(
                    simplex.dir.as_ref().unwrap(),
                    *simplex_id,
                )?
                .is_some_and(|rec| !rec.is_tombstoned());
                if vivant {
                    for &m in members {
                        crate::simplex_incidence::SimplexIncidenceZone::ensure_posted(
                            simplex.incidence.as_mut().unwrap(),
                            simplex.postings.as_mut().unwrap(),
                            m,
                            *simplex_id,
                        )?;
                    }
                }
            }
            Ok(Applied::Yes)
        }

        WalPayload::SimplexTombstone { simplex_id } => {
            simplex.open(substrate)?;
            // Idempotent : re-poser le flag est un no-op (retour ignore).
            let _ = crate::simplex_zone::SimplexDirZone::tombstone_raw(
                simplex.dir.as_mut().unwrap(),
                *simplex_id,
            )?;
            Ok(Applied::Yes)
        }

        // Configuration des passes simpliciales (T5) — etat absolu des
        // octets 16..26 de l'en-tete SPXD : rejouer reecrit les memes
        // octets, idempotent.
        WalPayload::SimplexPassesConfig {
            flags,
            clique_edge_type,
            clique_label_mask,
            dispatch_edge_type,
            dispatch_prop_key,
            dispatch_he_type,
        } => {
            simplex.open(substrate)?;
            let dir = simplex.dir.as_mut().unwrap();
            crate::simplex_zone::SimplexDirZone::init(dir)?;
            let dispatch = if *dispatch_edge_type == crate::simplex_passes::DISPATCH_ABSENT {
                None
            } else {
                Some(crate::simplex_passes::DispatchConfig {
                    edge_type: *dispatch_edge_type,
                    group_prop_key: *dispatch_prop_key,
                    he_type: *dispatch_he_type,
                })
            };
            crate::simplex_passes::write_config_raw(
                dir,
                &crate::simplex_passes::SimplexPassesConfig {
                    flags: *flags,
                    clique_edge_type: *clique_edge_type,
                    clique_label_mask: *clique_label_mask,
                    dispatch,
                },
            );
            Ok(Applied::Yes)
        }

        // Engram-membership side-table (T7).
        WalPayload::EngramMembersSet { engram_id, members } => {
            if engram_members_zone.is_none() {
                *engram_members_zone = Some(substrate.open_zone(Zone::EngramMembers)?);
            }
            let zf = engram_members_zone.as_mut().unwrap();
            crate::engram::EngramZone::set_members_raw(zf, *engram_id, members)?;
            Ok(Applied::Yes)
        }

        // Per-node engram bitset column (T7 Step 1).
        WalPayload::EngramBitsetSet { node_id, bitset } => {
            if engram_bitset_zone.is_none() {
                *engram_bitset_zone = Some(substrate.open_zone(Zone::EngramBitset)?);
            }
            let zf = engram_bitset_zone.as_mut().unwrap();
            crate::engram_bitset::EngramBitsetColumn::set_raw(zf, *node_id, *bitset)?;
            Ok(Applied::Yes)
        }

        // Hilbert page ordering (T11 Step 2).
        //
        // Idempotency story: the payload carries the `old_to_new`
        // permutation that was applied at first-sort time. We only
        // re-apply it if the substrate's meta header has NOT yet set
        // [`meta_flags::HILBERT_SORTED`] — i.e. we are replaying a WAL
        // segment that includes the sort event but the mmap state still
        // reflects the pre-sort layout (the case after a crash between
        // WAL-append and the subsequent meta-flag write).
        //
        // If the flag is already set, the mmap is already sorted (or
        // post-sort mutations landed on top) — re-applying would corrupt.
        // Silent skip is the correct behaviour. This is the Step 2
        // contract; the full transactional variant (T11 Step 4) uses an
        // out-of-place journal so the replay handler becomes a true
        // "re-run the whole thing" without pre-state assumptions.
        WalPayload::HilbertRepermute { permutation } => {
            let already_sorted =
                substrate.meta_header().flags & crate::meta::meta_flags::HILBERT_SORTED != 0;
            if already_sorted {
                return Ok(Applied::Skipped);
            }
            // La permutation réécrit tout le domaine [0, n) de la zone nodes :
            // une coquille peut atterrir n'importe où, l'intervalle ne borne plus.
            touches.noter_tout();
            // V1 : aucune annonce n'est posee sur ce chemin (l'epoch
            // n'existait pas encore), donc le drapeau n'a personne a
            // informer.
            apply_hilbert_permutation(substrate, nodes, edges, permutation, None)?;
            // Set the flag so a subsequent replay skips this record.
            // (The writer also sets it at the end of the sort operation;
            // we duplicate the write here so replay converges regardless
            // of which side crashed.)
            // The local header copy below is intentionally discarded (see
            // the persistence note that follows).
            #[allow(unused_variables, unused_assignments)]
            {
                let mut h = substrate.meta_header();
                h.flags |= crate::meta::meta_flags::HILBERT_SORTED;
            }
            // We cannot call `write_meta_header` on the immutable
            // `substrate` borrow here — so callers of `replay_from` that
            // care about the flag persisting to disk must re-open the
            // substrate as `&mut` or rely on the writer's own flag
            // write at end-of-sort. For Step 2, the flag persistence is
            // the writer's responsibility; replay only ensures the in-
            // memory state matches.
            Ok(Applied::Yes)
        }

        // Transactional per-community compaction (T11 Step 4).
        //
        // Mirrors [`Writer::compact_community`] phases 6-8 (node relocate, edge
        // endpoint remap, side-column rewrite). The WAL-first ordering in the
        // writer (record appended + fsync'd *before* any mmap mutation) lets
        // replay recover any crash between the log append and the completion
        // of phase 8 or the subsequent `msync` in phase 10.
        //
        // ## Idempotency contract
        //
        // We must handle three crash-window states uniformly:
        //
        //   * **A — pre-apply**: no relocation has landed. `node[old]` still
        //     carries `community_id == target`. Replay applies every `(old,
        //     new)` move.
        //   * **B — partial**: some relocations have landed (node copied to
        //     `new`, old slot zero-filled); others have not. Replay applies
        //     only the still-pending moves.
        //   * **C — post-apply**: every relocation has already been mirrored to
        //     the mmap. All old slots are zero-filled, no more edges reference
        //     the old domain. Replay is a no-op.
        //
        // The test `rec.community_id == target` at the *old* slot is the
        // discriminator: it holds in (A) and partially in (B), and fails in
        // (C) because the old slot was zero-filled on first apply (so its
        // `community_id` is 0, never equal to a live target ≥ 1). The writer
        // rejects `community_id == 0` at call time, so target ≥ 1 is invariant.
        //
        // Edges are naturally idempotent: `old_slots` is a closed set, and
        // once a `(src, dst)` pair has been remapped the new values live
        // outside that set, so `old_slots.contains(&rec.src)` is false on
        // subsequent passes and the remap becomes a no-op.
        //
        // ## Side-column caveats
        //
        // * `community_id` column — deterministic from the WAL payload
        //   (`target` cid at every new slot, 0 at every old slot). Always
        //   rewritten by replay.
        // * `hilbert_key` column — not recoverable from the WAL payload (the
        //   keys are a function of (centrality, degree) sampled at compaction
        //   time by `Writer::compact_community` phase 2). We zero the old
        //   entries for cleanliness but leave the new-slot entries untouched:
        //   if the writer's phase 8 completed before the crash, they carry
        //   the correct hkey; if not, they stay zero and the next
        //   `CommunityWarden` tick recomputes them. Hilbert is an
        //   *optimisation hint* (locality prefetch), not a correctness
        //   invariant — degraded locality is tolerable, incorrect topology
        //   is not.
        WalPayload::CompactCommunity {
            community_id,
            relocations,
            page_range: _,
        } => {
            // target cid 0 is meaningless — the writer rejects it, and letting
            // it through would break the "zero-filled slot ⇒ not-this-cid"
            // idempotency invariant.
            if *community_id == 0 || relocations.is_empty() {
                return Ok(Applied::Skipped);
            }

            // La compaction ne déplace que les slots de la communauté (old →
            // new) dans la zone nodes : borner l'audit à ces slots suffit, la
            // phase arêtes (phase 7) ne touchant pas la zone nodes.
            for &(old, new) in relocations {
                touches.noter(old);
                touches.noter(new);
            }

            // Open the five zones we touch (lazy, cached across records).
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            if edges.is_none() {
                *edges = Some(substrate.open_zone(Zone::Edges)?);
            }
            if hilbert.is_none() {
                *hilbert = Some(substrate.open_zone(Zone::Hilbert)?);
            }
            if community.is_none() {
                *community = Some(substrate.open_zone(Zone::Community)?);
            }
            if zorder.is_none() {
                *zorder = Some(substrate.open_zone(Zone::Zorder)?);
            }

            // Size-up every zone to hold the highest new slot.
            let max_new = relocations.iter().map(|(_, new)| *new).max().unwrap_or(0) as usize;
            ensure_room(
                nodes.as_mut().unwrap(),
                (max_new + 1) * NodeRecord::SIZE,
                1 << 20,
            )?;
            ensure_room(hilbert.as_mut().unwrap(), (max_new + 1) * 4, 1 << 20)?;
            ensure_room(community.as_mut().unwrap(), (max_new + 1) * 4, 1 << 20)?;
            ensure_room(zorder.as_mut().unwrap(), (max_new + 1) * 4, 1 << 20)?;

            // Lookup structures for the edge remap.
            let old_to_new: std::collections::HashMap<u32, u32> =
                relocations.iter().copied().collect();
            let old_slots: std::collections::HashSet<u32> =
                relocations.iter().map(|(old, _)| *old).collect();

            // ---- Phase 6 — node relocate (idempotent via cid-match) -------
            {
                let zf = nodes.as_mut().unwrap();
                // Snapshot still-pending moves so we can then mutate the zone
                // without aliasing the immutable read borrow.
                let mut pending: Vec<(u32, u32, NodeRecord)> =
                    Vec::with_capacity(relocations.len());
                {
                    let src = zf.as_slice();
                    for (old, new) in relocations.iter() {
                        let off = (*old as usize) * NodeRecord::SIZE;
                        if off + NodeRecord::SIZE > src.len() {
                            // Old slot never materialised — nothing to move.
                            continue;
                        }
                        let rec: &NodeRecord =
                            bytemuck::from_bytes(&src[off..off + NodeRecord::SIZE]);
                        if rec.community_id == *community_id {
                            pending.push((*old, *new, *rec));
                        }
                    }
                }
                let dst = zf.as_slice_mut();
                let zero = NodeRecord::default();
                for (old, new, rec) in &pending {
                    let new_off = (*new as usize) * NodeRecord::SIZE;
                    dst[new_off..new_off + NodeRecord::SIZE]
                        .copy_from_slice(bytemuck::bytes_of(rec));
                    let old_off = (*old as usize) * NodeRecord::SIZE;
                    dst[old_off..old_off + NodeRecord::SIZE]
                        .copy_from_slice(bytemuck::bytes_of(&zero));
                }
            }

            // ---- Phase 7 — edge endpoint remap (idempotent via old_slots) -
            {
                let zf = edges.as_mut().unwrap();
                let edge_slots = zf.as_slice().len() / EdgeRecord::SIZE;
                let dst = zf.as_slice_mut();
                for idx in 0..edge_slots {
                    let off = idx * EdgeRecord::SIZE;
                    let rec: EdgeRecord = *bytemuck::from_bytes(&dst[off..off + EdgeRecord::SIZE]);
                    if rec.is_tombstoned() {
                        continue;
                    }
                    let src_in = old_slots.contains(&rec.src);
                    let dst_in = old_slots.contains(&rec.dst);
                    if !src_in && !dst_in {
                        continue;
                    }
                    let mut updated = rec;
                    if src_in && let Some(&ns) = old_to_new.get(&rec.src) {
                        updated.src = ns;
                    }
                    if dst_in && let Some(&nd) = old_to_new.get(&rec.dst) {
                        updated.dst = nd;
                    }
                    dst[off..off + EdgeRecord::SIZE].copy_from_slice(bytemuck::bytes_of(&updated));
                }
            }

            // ---- Phase 8a — community side-column (deterministic) ---------
            {
                let zf = community.as_mut().unwrap();
                let bytes = zf.as_slice_mut();
                let cid_le = community_id.to_le_bytes();
                let zero_le = 0u32.to_le_bytes();
                for (_, new) in relocations.iter() {
                    let off = (*new as usize) * 4;
                    if off + 4 <= bytes.len() {
                        bytes[off..off + 4].copy_from_slice(&cid_le);
                    }
                }
                for (old, _) in relocations.iter() {
                    let off = (*old as usize) * 4;
                    if off + 4 <= bytes.len() {
                        bytes[off..off + 4].copy_from_slice(&zero_le);
                    }
                }
            }

            // ---- Phase 8b — Hilbert side-column (zero old slots only) -----
            //
            // We cannot recompute hilbert_keys at replay time (would need a
            // full centrality+degree pass). The writer's phase 8 has already
            // written the correct keys for new slots if it completed; if it
            // crashed before phase 8, the new-slot hilbert entries stay zero
            // and the next CommunityWarden tick will refresh them. Either way
            // the topology of nodes+edges is correct; only retrieval locality
            // is temporarily degraded.
            {
                let zf = hilbert.as_mut().unwrap();
                let bytes = zf.as_slice_mut();
                let zero_le = 0u32.to_le_bytes();
                for (old, _) in relocations.iter() {
                    let off = (*old as usize) * 4;
                    if off + 4 <= bytes.len() {
                        bytes[off..off + 4].copy_from_slice(&zero_le);
                    }
                }
            }

            // ---- Phase 8c — Z-order side-column (zero old slots only) -----
            //
            // Same hint semantics as the Hilbert column above: the zorder
            // keys are a function of (centrality, degree) sampled at
            // compaction time, not recoverable from the WAL payload. Zeroing
            // the old slots keeps the column clean; the new slots keep what
            // the writer's phase 8 wrote, and the CommunityWarden tick
            // refreshes everything on the next bulk sort.
            {
                let zf = zorder.as_mut().unwrap();
                let bytes = zf.as_slice_mut();
                let zero_le = 0u32.to_le_bytes();
                for (old, _) in relocations.iter() {
                    let off = (*old as usize) * 4;
                    if off + 4 <= bytes.len() {
                        bytes[off..off + 4].copy_from_slice(&zero_le);
                    }
                }
            }

            // Meta-header flag (HILBERT_SORTED clear) is the writer's
            // responsibility — same rationale as `HilbertRepermute` above:
            // the immutable `substrate` borrow here cannot rewrite the meta
            // header, and on next `open` the writer's phase 9 will either
            // have landed (flag clear) or replay will have reconstructed the
            // correct topology and the next compaction will clear the flag.

            Ok(Applied::Yes)
        }

        // ── Chantier SlotRemappable (2026-08-23) ────────────────────────
        //
        // V2 des deux repermutations : gardées par l'epoch durable de la
        // base (`substrate.remap.epoch`, écrivable ici contrairement au
        // méta), et enrôlant les side-cars slot-indexés. Chaque side-car à
        // en-tête se garde EN PLUS par son propre epoch + CRC, donc un
        // rejeu répété converge : premier passage = remap + estampille,
        // passages suivants = skip. Les zones engram ne sont pas touchées
        // ici — le writer a journalisé leurs états absolus
        // (`EngramMembersSet` / `EngramBitsetSet`) dans la même fenêtre de
        // durabilité que ce record ; ils suivent dans l'ordre des LSN.
        //
        // Fenêtre résiduelle documentée (héritée du design in-place de la
        // zone nœuds) : un crash entre le msync des zones et l'écriture de
        // l'epoch fait ré-appliquer la permutation nœuds/arêtes au rejeu
        // suivant (double application). La fermer exige la refonte
        // out-of-place journalisée de bulk_sort — hors périmètre.
        WalPayload::HilbertRepermuteV2 {
            permutation,
            epoch,
            engram_members,
            engram_bitsets,
        } => {
            let etat = crate::slot_remap::remap_epoch_state(substrate)?;

            // (1) Zones DÉJÀ permutées et confirmées : on ne re-permute
            //     pas, mais les side-cars peuvent être en retard (crash
            //     entre la confirmation et leur estampillage). Ils se
            //     gardent chacun par l'epoch de leur en-tête, donc les
            //     relancer est sûr et nécessaire.
            if etat.committed >= *epoch {
                crate::slot_remap::remap_sidecar_files_for_replay(substrate, permutation, *epoch)?;
                return Ok(Applied::Skipped);
            }

            // (2) Annonce sans confirmation : le processus est mort entre
            //     le début des mutations et la barrière de durabilité.
            //     Les zones peuvent être intactes, à moitié permutées ou
            //     entièrement permutées — RIEN ne les distingue. Deviner,
            //     c'est corrompre une fois sur deux, en silence et avec
            //     des CRC valides. On refuse, bruyamment.
            //     (2026-08-24, revue #2) VARIANTE DEDIEE, pas `Internal` :
            //     l'appelant (`SubstrateStore::from_substrate`) doit pouvoir
            //     DISTINGUER ce refus d'un WAL tronque par une panne, sans
            //     quoi il est avale par le `warn!` historique et la base
            //     s'ouvre quand meme sur des zones indeterminees.
            if etat.pending == *epoch {
                return Err(crate::error::SubstrateError::RepermutationIndeterminee {
                    epoch: *epoch,
                    base: substrate.path().display().to_string(),
                });
            }

            // (3) Cas nominal : les zones sont dans leur état
            //     pré-permutation (la fenêtre de rejeu était vide au
            //     moment du tri — garanti par
            //     `Writer::require_empty_replay_window`).
            //
            // ══ LE REJEU ANNONCE, LUI AUSSI (revue #4, 2026-08-24) ══
            //
            // Ce bras permutait les zones sans jamais poser d'annonce. Un
            // second arret DANS cette fenetre — entre la premiere mutation
            // et le `commit_remap_epoch` ci-dessous — laissait `committed`
            // inchange : le rejeu suivant retombait dans ce meme cas 3 et
            // RE-APPLIQUAIT la permutation. Noeuds a p², side-cars a p¹,
            // CRC valides, aucun symptome.
            //
            // Le commentaire qui ecartait cette fenetre comme « residuelle
            // et hors perimetre » datait de `ac0f3660`, AVANT que le couple
            // `pending`/`committed` n'existe. Il est faux depuis. Deux
            // lignes suffisent aujourd'hui a transformer une corruption
            // silencieuse en refus bruyant : l'annonce posee ici fera
            // tomber le rejeu suivant dans le cas (2) ci-dessus.
            // Cas (3) : permutation effective de tout le domaine [0, n) — une
            // coquille peut atterrir n'importe où, balayage complet requis.
            touches.noter_tout();
            let annonce = crate::slot_remap::AnnonceRemap::annoncer(substrate, *epoch)?;

            // ══ ET LA DISCIPLINE QUI VA AVEC (revue #5) ══
            //
            // Poser l'annonce sans cabler son effacement, c'est
            // transformer une correction en panne — ce module l'avait
            // ecrit noir sur blanc en revue #2, et ne l'avait applique
            // qu'au chemin vif. Sur CE chemin la faute coute plus cher :
            // le rejeu tourne A L'OUVERTURE, donc un ENOSPC passager au
            // demarrage briquait definitivement une base qu'il suffisait
            // avant de reessayer.
            //
            // Meme drapeau ternaire, meme handler — celui de
            // `slot_remap`, partage avec le writer, pour qu'il n'existe
            // pas deux exemplaires de cette regle.
            let mutations = (|| -> SubstrateResult<()> {
                apply_hilbert_permutation(substrate, nodes, edges, permutation, Some(&annonce))?;
                apply_engram_states(
                    substrate,
                    engram_members_zone,
                    engram_bitset_zone,
                    engram_members,
                    engram_bitsets,
                )?;
                for z in [
                    nodes.as_ref(),
                    edges.as_ref(),
                    engram_members_zone.as_ref(),
                    engram_bitset_zone.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    z.msync()?;
                    z.fsync()?;
                }
                // ══ BARRIERE FRANCHIE ══
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
            crate::slot_remap::remap_sidecar_files_for_replay(substrate, permutation, *epoch)?;
            Ok(Applied::Yes)
        }

        WalPayload::CompactCommunityV2 {
            community_id,
            relocations,
            page_range,
            epoch,
            engram_members,
            engram_bitsets,
        } => {
            if *community_id == 0 || relocations.is_empty() {
                return Ok(Applied::Skipped);
            }
            // PAS de garde d'epoch sur les phases de zones ici, à la
            // différence de `HilbertRepermuteV2` : la relocation par
            // communauté est idempotente PAR CONSTRUCTION (discriminant
            // `community_id == cible` au vieux slot, ensemble fermé
            // `old_slots` pour les arêtes). Elle est donc auto-corrective
            // — y compris quand un rejeu depuis 0 vient de re-matérialiser
            // les vieux slots via les NodeInsert antérieurs, cas où une
            // garde d'epoch aurait laissé les nœuds en arrière pendant que
            // les side-cars restaient compactés.
            //
            // Phases nœuds / arêtes / side-colonnes : identiques à la V1 —
            // délégation au bras V1 via un record synthétique.
            let v1 = WalRecord {
                lsn: rec.lsn,
                timestamp: rec.timestamp,
                flags: rec.flags,
                payload: WalPayload::CompactCommunity {
                    community_id: *community_id,
                    relocations: relocations.clone(),
                    page_range: *page_range,
                },
            };
            apply(
                substrate,
                &v1,
                nodes,
                edges,
                engram_members_zone,
                engram_bitset_zone,
                hilbert,
                community,
                zorder,
                simplex,
                touches,
            )?;
            apply_engram_states(
                substrate,
                engram_members_zone,
                engram_bitset_zone,
                engram_members,
                engram_bitsets,
            )?;
            // Side-cars : permutation totale équivalente (identité +
            // échanges old ↔ new), auto-gardée par en-tête.
            let domain = relocations
                .iter()
                .map(|&(old, new)| old.max(new))
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            let total_perm = crate::slot_remap::permutation_from_relocations(relocations, domain)?;
            // I3 — barrière de durabilité AVANT la confirmation, puis
            // side-cars (auto-gardés par leurs epochs d'en-tête).
            for z in [
                nodes.as_ref(),
                edges.as_ref(),
                hilbert.as_ref(),
                community.as_ref(),
                engram_members_zone.as_ref(),
                engram_bitset_zone.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                z.msync()?;
                z.fsync()?;
            }
            crate::slot_remap::commit_remap_epoch(substrate, *epoch)?;
            crate::slot_remap::remap_sidecar_files_for_replay(substrate, &total_perm, *epoch)?;
            Ok(Applied::Yes)
        }

        WalPayload::NodePropHeadUpdate {
            node_id,
            first_prop_off,
        } => {
            touches.noter(*node_id);
            if nodes.is_none() {
                *nodes = Some(substrate.open_zone(Zone::Nodes)?);
            }
            let zf = nodes.as_mut().unwrap();
            let record_offset = (*node_id as usize) * NodeRecord::SIZE;
            let field_offset = record_offset + 14; // first_prop_off byte offset
            if field_offset + 6 > zf.as_slice().len() {
                // Slot not yet materialised — the matching NodeInsert has
                // not been replayed yet, so there is nothing to anchor this
                // head pointer to. The next iteration of the replay loop
                // will typically visit the NodeInsert first (records are
                // replayed in LSN order), at which point a later
                // NodePropHeadUpdate re-applies on the now-materialised
                // slot. Tolerated as a no-op here.
                return Ok(Applied::Yes);
            }
            let u48 = U48::from_u64(*first_prop_off);
            zf.as_slice_mut()[field_offset..field_offset + 6].copy_from_slice(&u48.0);
            Ok(Applied::Yes)
        }

        WalPayload::EdgePropHeadUpdate {
            edge_idx,
            first_prop_off,
        } => {
            if edges.is_none() {
                *edges = Some(substrate.open_zone(Zone::Edges)?);
            }
            let zf = edges.as_mut().unwrap();
            let record_offset = (*edge_idx as usize) * EdgeRecord::SIZE;
            let field_offset = record_offset + EdgeRecord::FIRST_PROP_OFF_BYTES;
            if field_offset + 6 > zf.as_slice().len() {
                // Slot not yet materialised — the matching EdgeInsert has
                // not been replayed yet, so there is nothing to anchor this
                // head pointer to. Tolerated as a no-op here; a later
                // EdgePropHeadUpdate record (records are replayed in LSN
                // order) re-applies on the now-materialised slot.
                return Ok(Applied::Yes);
            }
            let u48 = U48::from_u64(*first_prop_off);
            zf.as_slice_mut()[field_offset..field_offset + 6].copy_from_slice(&u48.0);
            Ok(Applied::Yes)
        }

        // Markers — no mutation.
        WalPayload::NoOp | WalPayload::EndOfLog | WalPayload::Checkpoint { .. } => Ok(Applied::Yes),
        // (2026-11-01, D-DUR) Frontières de transaction : lues par la boucle
        // de `replay_from` (registre `en_vol`), rien à appliquer aux zones.
        WalPayload::TxNoeudEnAttente { .. }
        | WalPayload::TxAreteEnAttente { .. }
        | WalPayload::TxCommit { .. } => Ok(Applied::Yes),

        // ── VARIANTES DELIBEREMENT NON REJOUEES ──────────────────────────
        //
        // (2026-08-14) CETTE LISTE ETAIT UN `_ => Skipped`. Un fourre-tout
        // silencieux : treize variantes y tombaient, dont les quatre familles
        // de proprietes — et rien ne le disait. Le jour ou quelqu'un ajoute
        // une variante au WAL, elle disparait sans un mot, et le rejeu perd
        // une donnee de plus sans que la compilation bronche.
        //
        // La liste est desormais EXHAUSTIVE et NOMMEE. Une variante ajoutee
        // demain CASSE LA COMPILATION ici — c'est la seule garantie qui
        // survive a l'oubli ; tout le reste suppose quelqu'un qui pense a
        // relire.
        //
        // Chaque ligne porte la RAISON du saut. « Non implemente » n'en est
        // pas une : ou la donnee est reconstructible, ou elle est rejouee
        // ailleurs, ou c'est un trou qu'il faut nommer comme tel.

        // Rejouees AILLEURS, par `SubstrateStore::merge_wal_inline_props` :
        // elles atterrissent dans les cartes en memoire (`node_properties`,
        // `edge_properties`), que ce module — qui ne voit que le fichier et
        // ses zones — ne peut pas atteindre. Les traiter ici serait les
        // appliquer deux fois, ou pire, les appliquer a moitie.
        WalPayload::PropSetInline { .. }
        | WalPayload::PropRemoveInline { .. }
        | WalPayload::EdgePropSetInline { .. }
        | WalPayload::EdgePropRemoveInline { .. } => Ok(Applied::Skipped),

        // Forme HERITEE des proprietes, adressee par `key_id`. Remplacee par
        // les variantes `*Inline`, qui portent le nom de la cle et ne
        // dependent donc pas d'un registre restaure au prealable.
        WalPayload::PropSet { .. } | WalPayload::PropDelete { .. } => Ok(Applied::Skipped),

        // Registres. Ils vivent dans la zone dict, ecrite par `persist_dict`
        // au flush — MEME FENETRE DE PERTE que les proprietes.
        // TROU CONNU, ET C'EN EST UN VRAI : un `label_bitset` restaure peut
        // referencer un bit dont le nom est perdu. Le rejeu se fait cote
        // store (`merge_wal_inline_props`), qui possede les registres.
        // (2026-08-15) Les Intern ne touchent pas les ZONES : ils sont
        // appliques aux registres par merge_wal_inline_props, qui tourne
        // apres ce rejeu dans l ordre d ouverture. Ici : sautes, comptes.
        WalPayload::LabelIntern { .. }
        | WalPayload::KeyIntern { .. }
        | WalPayload::EdgeTypeIntern { .. }
        | WalPayload::StringIntern { .. } => Ok(Applied::Skipped),

        // Donnees DERIVEES : entierement reconstructibles depuis le graphe.
        // Les perdre coute un recalcul, jamais une information.
        // Aretes inter-bases (V1) : elles vivent dans un sidecar DashMap du
        // store, pas dans les zones — rejouees par
        // `SubstrateStore::merge_wal_external_edges` a l'ouverture.
        WalPayload::ExternalEdgeInsert { .. } | WalPayload::ExternalEdgeDelete { .. } => {
            Ok(Applied::Skipped)
        }

        // (O-VEC C3) Marqueur d'écriture vectorielle : la colonne de
        // vecteurs fait foi, le rejeu n'a rien à appliquer. Il n'est pas
        // « non supporté » — il a fait son office en déplaçant l'offset.
        WalPayload::VecColumnWrite { .. } => Ok(Applied::Yes),
        // (obrain#120) Les colonnes blob sont rejouées par
        // `SubstrateStore::merge_wal_inline_props`, qui tient le registre des
        // colonnes ; ce rejeu-ci ne porte que les zones.
        WalPayload::BlobColumnWrite { .. } | WalPayload::BlobColumnClear { .. } => {
            Ok(Applied::Skipped)
        }

        WalPayload::RicciUpdate { .. }
        | WalPayload::Tier0Update { .. }
        | WalPayload::Tier1Update { .. }
        | WalPayload::Tier2Update { .. } => Ok(Applied::Skipped),
    }
}

/// Chantier SlotRemappable — applique les états ABSOLUS des zones engram
/// portés par un record de repermutation. Idempotent par nature (ce sont
/// des états complets, pas des deltas), et gardé par la même condition
/// que la permutation des zones puisqu'il voyage dans le même record.
fn apply_engram_states(
    substrate: &SubstrateFile,
    engram_members_zone: &mut Option<ZoneFile>,
    engram_bitset_zone: &mut Option<ZoneFile>,
    members: &[(u16, Vec<u32>)],
    bitsets: &[(u32, u64)],
) -> SubstrateResult<()> {
    if !members.is_empty() {
        if engram_members_zone.is_none() {
            *engram_members_zone = Some(substrate.open_zone(Zone::EngramMembers)?);
        }
        let zf = engram_members_zone.as_mut().unwrap();
        for (engram_id, list) in members {
            crate::engram::EngramZone::set_members_raw(zf, *engram_id, list)?;
        }
    }
    if !bitsets.is_empty() {
        if engram_bitset_zone.is_none() {
            *engram_bitset_zone = Some(substrate.open_zone(Zone::EngramBitset)?);
        }
        let zf = engram_bitset_zone.as_mut().unwrap();
        for (node_id, bits) in bitsets {
            crate::engram_bitset::EngramBitsetColumn::set_raw(zf, *node_id, *bits)?;
        }
    }
    Ok(())
}

/// Corps commun des bras `HilbertRepermute` (V1) et `HilbertRepermuteV2` :
/// permutation out-of-place de la zone Nodes + réécriture des `src`/`dst`
/// d'arêtes. Le gating (flag `HILBERT_SORTED` pour la V1, epoch durable
/// pour la V2) reste dans les bras appelants.
/// `phase` est pose a `AvantBarriere` **sur la ligne meme** de la
/// premiere ecriture d'octet, jamais deduit de la progression : c'est ce
/// qui permet a l'appelant de distinguer « ENOSPC avant d'avoir rien
/// touche » (zones prouvees intactes, on efface l'annonce) de « ENOSPC
/// apres » (etat indetermine, on refuse). Voir
/// `slot_remap::abandonner_annonce`.
pub(crate) fn apply_hilbert_permutation(
    substrate: &SubstrateFile,
    nodes: &mut Option<ZoneFile>,
    edges: &mut Option<ZoneFile>,
    permutation: &[u32],
    annonce: Option<&crate::slot_remap::AnnonceRemap<'_>>,
) -> SubstrateResult<()> {
    let n = permutation.len();

    // Permute nodes out-of-place.
    if nodes.is_none() {
        *nodes = Some(substrate.open_zone(Zone::Nodes)?);
    }
    let zf = nodes.as_mut().unwrap();
    let total = n * NodeRecord::SIZE;
    ensure_room(zf, total, 1 << 20)?;
    let src: Vec<NodeRecord> =
        bytemuck::cast_slice::<u8, NodeRecord>(&zf.as_slice()[..total]).to_vec();
    let mut dst = vec![NodeRecord::default(); n];
    for old in 0..n {
        let new = permutation[old] as usize;
        if new < n {
            dst[new] = src[old];
        }
    }
    // ── PREMIERE MUTATION DE ZONE ──
    // `ensure_room` ci-dessus peut echouer sans avoir touche un octet :
    // c'est LA fenetre que ce drapeau protege.
    if let Some(a) = annonce {
        a.premiere_mutation();
    }
    zf.as_slice_mut()[..total].copy_from_slice(bytemuck::cast_slice(&dst));

    // Rewrite edge.src/dst in place.
    if edges.is_none() {
        *edges = Some(substrate.open_zone(Zone::Edges)?);
    }
    let ezf = edges.as_mut().unwrap();
    let edge_slots = ezf.len() as usize / EdgeRecord::SIZE;
    for idx in 0..edge_slots {
        let offset = idx * EdgeRecord::SIZE;
        let rec: EdgeRecord =
            *bytemuck::from_bytes(&ezf.as_slice()[offset..offset + EdgeRecord::SIZE]);
        if rec.is_tombstoned() {
            continue;
        }
        let mut updated = rec;
        if (rec.src as usize) < n {
            updated.src = permutation[rec.src as usize];
        }
        if (rec.dst as usize) < n {
            updated.dst = permutation[rec.dst as usize];
        }
        ezf.as_slice_mut()[offset..offset + EdgeRecord::SIZE]
            .copy_from_slice(bytemuck::bytes_of(&updated));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::SubstrateFile;
    use crate::record::{EdgeRecord, NodeRecord, PackedScarUtilAff, U48, f32_to_q1_15};
    use crate::wal_io::SyncMode;
    use crate::writer::Writer;
    use tempfile::tempdir;

    fn n(_i: u32, label: u64) -> NodeRecord {
        NodeRecord {
            label_bitset: label,
            first_edge_off: U48::default(),
            first_prop_off: U48::default(),
            community_id: 0,
            energy: f32_to_q1_15(0.5),
            scar_util_affinity: PackedScarUtilAff::new(0, 0, 0, false).pack(),
            centrality_cached: 0,
            flags: 0,
            // (jalon 2) appartenance : fixture publique.
            tenant_id: 0,
            _reserve: 0,
        }
    }

    fn e(_i: u64, src: u32, dst: u32) -> EdgeRecord {
        EdgeRecord {
            src,
            dst,
            edge_type: 1,
            weight_u16: f32_to_q1_15(0.8),
            next_from: U48::default(),
            next_to: U48::default(),
            first_prop_off: U48::ZERO,
            ricci_u8: 0,
            flags: 0,
            engram_tag: 0,
            _pad: [0; 2],
        }
    }

    #[test]
    fn replay_empty_wal_is_noop() {
        let dir = tempdir().unwrap();
        let sub = SubstrateFile::create(dir.path().join("kb")).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(stats.applied, 0);
        assert_eq!(stats.decode_errors, 0);
    }

    #[test]
    fn replay_reconstructs_nodes_and_edges() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");

        // Round 1: create substrate, write some records, commit, drop without
        // flushing the mmap — simulate a crash that left the WAL durable but
        // the mmap writes may or may not have reached disk.
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            for i in 0..10u32 {
                w.write_node(i, n(i, 1 << (i % 8))).unwrap();
            }
            for i in 0..5u64 {
                w.write_edge(i, e(i, i as u32, (i + 1) as u32)).unwrap();
            }
            w.commit().unwrap();
            // no msync_zones() — pretend we crashed before flushing.
        }

        // Round 2: wipe the zone files to prove replay reconstructs from WAL.
        std::fs::write(sub_path.join(crate::file::zone::NODES), []).unwrap();
        std::fs::write(sub_path.join(crate::file::zone::EDGES), []).unwrap();

        // Round 3: reopen + replay.
        let sub2 = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub2, 0).unwrap();
        assert!(stats.applied >= 15); // 10 nodes + 5 edges + 1 commit marker
        assert_eq!(stats.decode_errors, 0);

        // Verify the nodes zone now reflects the inserts.
        let nz = sub2.open_zone(Zone::Nodes).unwrap();
        assert!(nz.len() >= 10 * NodeRecord::SIZE as u64);
        let slice: &[NodeRecord] = bytemuck::cast_slice(&nz.as_slice()[..10 * NodeRecord::SIZE]);
        for i in 0..10u32 {
            assert_eq!(slice[i as usize].label_bitset, 1u64 << (i % 8));
        }
        let ez = sub2.open_zone(Zone::Edges).unwrap();
        let eslice: &[EdgeRecord] = bytemuck::cast_slice(&ez.as_slice()[..5 * EdgeRecord::SIZE]);
        for i in 0..5u64 {
            assert_eq!(eslice[i as usize].src, i as u32);
            assert_eq!(eslice[i as usize].dst, (i + 1) as u32);
        }
    }

    #[test]
    fn replay_is_idempotent() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            for i in 0..5u32 {
                w.write_node(i, n(i, i as u64 + 1)).unwrap();
            }
            w.commit().unwrap();
        }

        let sub = SubstrateFile::open(&sub_path).unwrap();
        let s1 = replay_from(&sub, 0).unwrap();
        let s2 = replay_from(&sub, 0).unwrap();
        // Applied counts match — same records, applied the same way.
        assert_eq!(s1.applied, s2.applied);
        assert_eq!(s1.decode_errors, 0);
        assert_eq!(s2.decode_errors, 0);

        // Bytes still correct.
        let nz = sub.open_zone(Zone::Nodes).unwrap();
        let slice: &[NodeRecord] = bytemuck::cast_slice(&nz.as_slice()[..5 * NodeRecord::SIZE]);
        for i in 0..5u32 {
            assert_eq!(slice[i as usize].label_bitset, i as u64 + 1);
        }
    }

    #[test]
    fn replay_stops_at_crc_boundary() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        let wal_path;
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            wal_path = sub.wal_path();
            let w = Writer::new(sub, SyncMode::Never).unwrap();
            for i in 0..10u32 {
                w.write_node(i, n(i, i as u64 + 1)).unwrap();
            }
            w.commit().unwrap();
            w.wal().fsync().unwrap();
        }

        // Tamper the middle of the WAL to simulate a torn write.
        let mut bytes = std::fs::read(&wal_path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xFF;
        std::fs::write(&wal_path, bytes).unwrap();

        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(stats.decode_errors, 1);
        assert!(stats.applied > 0);
        assert!(stats.applied < 11); // stopped before all records
    }

    // =====================================================================
    // T6 cognitive-column replay
    // =====================================================================

    #[test]
    fn replay_reapplies_energy_reinforce_and_decay() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");

        // Round 1: write 3 nodes, seed their energy columns via EnergyReinforce
        // (NodeInsert intentionally carries only label_bitset — cognitive
        // columns always transit via dedicated payloads, that's the T6
        // invariant), then apply decay ×0.5, tombstone one of them.
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            for i in 1..=3u32 {
                w.write_node(i, n(i, 1)).unwrap();
                // Seed energy to 0.4 via EnergyReinforce.
                w.reinforce_energy(i, crate::record::f32_to_q1_15(0.4))
                    .unwrap();
            }
            // Reinforce node 2 to 0.9.
            w.reinforce_energy(2, crate::record::f32_to_q1_15(0.9))
                .unwrap();
            // Decay all by ×0.5.
            w.decay_all_energy(32768, 4).unwrap();
            // Tombstone node 3 after decay.
            w.tombstone_node(3).unwrap();
            w.commit().unwrap();
            w.wal().fsync().unwrap();
        }

        // Round 2: wipe the nodes zone.
        std::fs::write(sub_path.join(crate::file::zone::NODES), []).unwrap();

        // Round 3: replay rebuilds the zone AND reapplies column mutations.
        let sub2 = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub2, 0).unwrap();
        assert_eq!(stats.decode_errors, 0);
        assert!(stats.applied > 0);

        let nz = sub2.open_zone(Zone::Nodes).unwrap();
        let slice: &[NodeRecord] = bytemuck::cast_slice(&nz.as_slice()[..4 * NodeRecord::SIZE]);

        // Node 1: seed 0.4 → decay ×0.5 → 0.2 (±1 ULP).
        let e1 = crate::record::q1_15_to_f32(slice[1].energy);
        assert!((e1 - 0.2).abs() < 1e-3, "node 1 energy after replay: {e1}");
        // Node 2: seed 0.4 → reinforce 0.9 → decay ×0.5 → 0.45.
        let e2 = crate::record::q1_15_to_f32(slice[2].energy);
        assert!((e2 - 0.45).abs() < 1e-3, "node 2 energy after replay: {e2}");
        // Node 3: tombstoned after decay — energy ≈ 0.2, TOMBSTONED flag set.
        assert!(slice[3].is_tombstoned(), "node 3 must be tombstoned");
    }

    #[test]
    fn replay_reapplies_synapse_reinforce_and_scar_util() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            w.write_node(1, n(1, 1)).unwrap();
            w.write_edge(1, e(1, 1, 2)).unwrap();
            // Reinforce synapse weight to 0.77.
            let weight = (0.77_f32 * 65535.0).round() as u16;
            w.reinforce_synapse(1, weight).unwrap();
            // Set packed scar/util/affinity.
            let packed = crate::record::PackedScarUtilAff::new(9, 13, 25, false).pack();
            w.set_scar_util_affinity(1, packed).unwrap();
            // Centrality batch.
            w.update_centrality_batch(vec![(1, 0xABCD)]).unwrap();
            w.commit().unwrap();
            w.wal().fsync().unwrap();
        }
        // Wipe both zones.
        std::fs::write(sub_path.join(crate::file::zone::NODES), []).unwrap();
        std::fs::write(sub_path.join(crate::file::zone::EDGES), []).unwrap();

        let sub2 = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub2, 0).unwrap();
        assert_eq!(stats.decode_errors, 0);

        let nz = sub2.open_zone(Zone::Nodes).unwrap();
        let n_slice: &[NodeRecord] = bytemuck::cast_slice(&nz.as_slice()[..2 * NodeRecord::SIZE]);
        let unpacked = crate::record::PackedScarUtilAff::unpack(n_slice[1].scar_util_affinity);
        assert_eq!(unpacked.scar, 9);
        assert_eq!(unpacked.utility, 13);
        assert_eq!(unpacked.affinity, 25);
        assert_eq!(n_slice[1].centrality_cached, 0xABCD);

        let ez = sub2.open_zone(Zone::Edges).unwrap();
        let e_slice: &[EdgeRecord] = bytemuck::cast_slice(&ez.as_slice()[..2 * EdgeRecord::SIZE]);
        let expected_weight = (0.77_f32 * 65535.0).round() as u16;
        assert_eq!(e_slice[1].weight_u16, expected_weight);
    }

    // =====================================================================
    // T11 Step 4 — transactional community compaction replay
    //
    // Acceptance criterion (plan): "Test crash mid-compaction → reopen →
    // replay → état cohérent (soit pré-, soit post-compaction, jamais
    // partiel)." We test three crash windows that together cover the full
    // state space of the write protocol:
    //
    //   1. pre-apply   — WAL has the record, mmap is unchanged.
    //   2. partial     — WAL has the record, mmap reflects some but not all
    //                    of phases 6-8.
    //   3. post-apply  — writer ran end to end, replay must not corrupt the
    //                    post-compaction state.
    //
    // All three must converge to the *same* post-compaction state.
    // =====================================================================

    /// Helper: node record pre-tagged with a community_id.
    fn n_cid(i: u32, label: u64, cid: u32) -> NodeRecord {
        let mut rec = n(i, label);
        rec.community_id = cid;
        rec
    }

    /// Helper: seed a substrate with a 3-node, 3-edge community tagged
    /// `cid`, plus one external node (slot 42) with one cross-community
    /// edge. Used by the three compaction tests.
    ///
    /// After this returns, the WAL contains the full pre-compaction history
    /// (NodeInsert + NodeUpdate + EdgeInsert records) and the mmap is
    /// fsync'd to match. The caller can then append a CompactCommunity
    /// record (or run `compact_community`) to set up the specific crash
    /// window under test.
    fn seed_community(sub_path: &std::path::Path, old_slots: &[u32; 3], cid: u32) -> Writer {
        let sub = SubstrateFile::create(sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        for (i, &slot) in old_slots.iter().enumerate() {
            // NodeInsert only records label_bitset; NodeUpdate threads the
            // community_id through the WAL so replay can reconstruct the
            // pre-compaction cid assignment.
            w.write_node(slot, n_cid(slot, (i as u64) + 1, cid))
                .unwrap();
            w.update_node(slot, n_cid(slot, (i as u64) + 1, cid))
                .unwrap();
        }
        // Intra-community edges.
        w.write_edge(0, e(0, old_slots[0], old_slots[1])).unwrap();
        w.write_edge(1, e(1, old_slots[1], old_slots[2])).unwrap();
        // External node + cross-community edge.
        w.write_node(42, n(42, 0xFF)).unwrap();
        w.write_edge(2, e(2, old_slots[0], 42)).unwrap();
        w.commit().unwrap();
        // Flush the pre-compaction state so it survives a reopen — this is
        // the mmap state "just before the crash" for tests that inject the
        // CompactCommunity record manually.
        w.msync_zones().unwrap();
        // Chantier SlotRemappable — ce helper simule un store qui vient de
        // flusher : on publie donc la baseline correspondante, sans quoi
        // `compact_community` refuse (fenetre de rejeu non vide).
        w.note_durable_baseline(w.wal().offset());
        w
    }

    /// Shared assertion: after replay, verify the post-compaction state
    /// (old slots zeroed, new slots filled, edges remapped, community col
    /// coherent).
    fn assert_post_compaction_state(
        sub: &SubstrateFile,
        old_slots: &[u32; 3],
        new_slots: &[u32; 3],
        cid: u32,
    ) {
        // Nodes.
        let nz = sub.open_zone(Zone::Nodes).unwrap();
        let need = (new_slots[2] as usize + 1) * NodeRecord::SIZE;
        let nodes: &[NodeRecord] = bytemuck::cast_slice(&nz.as_slice()[..need]);
        for &old in old_slots {
            let rec = &nodes[old as usize];
            assert_eq!(
                rec.community_id, 0,
                "old slot {old} community_id must be 0 after compaction"
            );
            assert_eq!(
                rec.label_bitset, 0,
                "old slot {old} label_bitset must be 0 after compaction"
            );
        }
        // Reconstitue le mapping old→new PAR LABEL (robuste à l'ordre de tri
        // interne de `compact_community`, qui dépend du degré réel depuis
        // Step 7 — les clés ne sont plus à égalité systématique).
        let mut label_to_new: std::collections::HashMap<u64, u32> =
            std::collections::HashMap::new();
        for &new in new_slots {
            let rec = &nodes[new as usize];
            assert_eq!(rec.community_id, cid, "new slot {new} must carry cid={cid}");
            assert!(
                (1..=(old_slots.len() as u64)).contains(&rec.label_bitset),
                "new slot {new}: label {} hors {1:?}..={}",
                rec.label_bitset,
                old_slots.len()
            );
            assert!(
                label_to_new.insert(rec.label_bitset, new).is_none(),
                "label {} présent deux fois parmi les nouveaux slots",
                rec.label_bitset
            );
        }
        let mut map_old_new: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for (i, &old) in old_slots.iter().enumerate() {
            let label = (i as u64) + 1;
            let new = *label_to_new
                .get(&label)
                .expect("chaque label d'ancien slot doit avoir un nouveau slot");
            map_old_new.insert(old, new);
        }
        // External node untouched.
        assert_eq!(nodes[42].label_bitset, 0xFF);
        assert_eq!(nodes[42].community_id, 0);

        // Edges (endpoints d'origine reconstruits depuis la structure du
        // fixture : old[0]→old[1], old[1]→old[2], old[0]→42).
        let m = |old: u32| map_old_new[&old];
        let ez = sub.open_zone(Zone::Edges).unwrap();
        let edges: &[EdgeRecord] = bytemuck::cast_slice(&ez.as_slice()[..3 * EdgeRecord::SIZE]);
        assert_eq!(edges[0].src, m(old_slots[0]), "edge 0 src remapped");
        assert_eq!(edges[0].dst, m(old_slots[1]), "edge 0 dst remapped");
        assert_eq!(edges[1].src, m(old_slots[1]), "edge 1 src remapped");
        assert_eq!(edges[1].dst, m(old_slots[2]), "edge 1 dst remapped");
        // Cross-community edge: only the in-community endpoint is remapped.
        assert_eq!(edges[2].src, m(old_slots[0]), "edge 2 src remapped");
        assert_eq!(edges[2].dst, 42, "edge 2 dst (external) untouched");

        // Community side-column.
        let cz = sub.open_zone(Zone::Community).unwrap();
        let cbytes = cz.as_slice();
        for &old in old_slots {
            let off = (old as usize) * 4;
            if off + 4 <= cbytes.len() {
                let v = u32::from_le_bytes(cbytes[off..off + 4].try_into().unwrap());
                assert_eq!(v, 0, "community col at old slot {old} must be 0");
            }
        }
        for &new in new_slots {
            let off = (new as usize) * 4;
            assert!(
                off + 4 <= cbytes.len(),
                "community zone must be sized up to cover new slot {new}"
            );
            let v = u32::from_le_bytes(cbytes[off..off + 4].try_into().unwrap());
            assert_eq!(v, cid, "community col at new slot {new} must be {cid}");
        }
    }

    /// Crash window **A — pre-apply**: the CompactCommunity record is
    /// durable in the WAL but none of phases 6-8 have touched the mmap.
    /// Replay must apply the full compaction.
    #[test]
    fn replay_compact_community_pre_apply() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        let cid = 7u32;
        let old_slots = [1u32, 2, 3];
        let new_slots = [100u32, 101, 102];

        {
            let w = seed_community(&sub_path, &old_slots, cid);

            // Manually append CompactCommunity to simulate a crash between
            // WAL-append (phase 5) and the first mmap mutation (phase 6).
            let relocations: Vec<(u32, u32)> = old_slots
                .iter()
                .zip(new_slots.iter())
                .map(|(&o, &nn)| (o, nn))
                .collect();
            let rec = WalRecord {
                lsn: 0,
                timestamp: 0,
                flags: 0,
                payload: WalPayload::CompactCommunity {
                    community_id: cid,
                    relocations,
                    page_range: (0, 1),
                },
            };
            w.wal().append(rec).unwrap();
            w.commit().unwrap();
            w.wal().fsync().unwrap();
            // NOTE: no msync_zones after the compact record — the mmap
            // reflects the pre-compaction state, exactly as it would after
            // a crash mid-transaction.
        }

        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(stats.decode_errors, 0);
        // 3 NodeInsert + 3 NodeUpdate + 1 NodeInsert(42) + 3 EdgeInsert
        // + 1 CompactCommunity + commit markers — all applied cleanly.
        assert!(stats.applied > 0);

        assert_post_compaction_state(&sub, &old_slots, &new_slots, cid);
    }

    /// Crash window **B — partial-apply**: the writer got past phase 6
    /// for *some* relocations but not all. The mmap is in an inconsistent
    /// intermediate state (some nodes moved, some still at old slots; some
    /// community-col entries rewritten, some not). Replay must complete
    /// the compaction.
    ///
    /// We simulate the partial state by opening the zones directly and
    /// applying the first two relocations (1→100, 2→101) while leaving
    /// the third (3→102) untouched.
    #[test]
    fn replay_compact_community_partial_apply() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        let cid = 7u32;
        let old_slots = [1u32, 2, 3];
        let new_slots = [100u32, 101, 102];

        {
            let w = seed_community(&sub_path, &old_slots, cid);

            let relocations: Vec<(u32, u32)> = old_slots
                .iter()
                .zip(new_slots.iter())
                .map(|(&o, &nn)| (o, nn))
                .collect();
            let rec = WalRecord {
                lsn: 0,
                timestamp: 0,
                flags: 0,
                payload: WalPayload::CompactCommunity {
                    community_id: cid,
                    relocations: relocations.clone(),
                    page_range: (0, 1),
                },
            };
            w.wal().append(rec).unwrap();
            w.commit().unwrap();
            w.wal().fsync().unwrap();

            // --- Simulate partial phase 6 (first 2 of 3 relocations) -----
            let sub_arc = w.substrate();
            let sub = sub_arc.lock();
            let mut nz = sub.open_zone(Zone::Nodes).unwrap();
            let mut cz = sub.open_zone(Zone::Community).unwrap();
            crate::writer::ensure_room(
                &mut nz,
                (new_slots[2] as usize + 1) * NodeRecord::SIZE,
                1 << 20,
            )
            .unwrap();
            crate::writer::ensure_room(&mut cz, (new_slots[2] as usize + 1) * 4, 1 << 20).unwrap();
            // Partial relocation: first two pairs.
            for (old, new) in relocations.iter().take(2) {
                let old_off = (*old as usize) * NodeRecord::SIZE;
                let new_off = (*new as usize) * NodeRecord::SIZE;
                let rec_bytes: [u8; NodeRecord::SIZE] = {
                    let slice = &nz.as_slice()[old_off..old_off + NodeRecord::SIZE];
                    let mut out = [0u8; NodeRecord::SIZE];
                    out.copy_from_slice(slice);
                    out
                };
                let zero = NodeRecord::default();
                nz.as_slice_mut()[new_off..new_off + NodeRecord::SIZE].copy_from_slice(&rec_bytes);
                nz.as_slice_mut()[old_off..old_off + NodeRecord::SIZE]
                    .copy_from_slice(bytemuck::bytes_of(&zero));
                // Community col partial rewrite.
                let cid_le = cid.to_le_bytes();
                let zero_le = 0u32.to_le_bytes();
                cz.as_slice_mut()[(*new as usize) * 4..(*new as usize) * 4 + 4]
                    .copy_from_slice(&cid_le);
                cz.as_slice_mut()[(*old as usize) * 4..(*old as usize) * 4 + 4]
                    .copy_from_slice(&zero_le);
            }
            nz.msync().unwrap();
            nz.fsync().unwrap();
            cz.msync().unwrap();
            cz.fsync().unwrap();
            drop(sub);
        }

        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(stats.decode_errors, 0);

        assert_post_compaction_state(&sub, &old_slots, &new_slots, cid);
    }

    /// Crash window **C — post-apply**: the writer completed all phases
    /// successfully. The mmap is in the post-compaction state and the WAL
    /// carries the CompactCommunity record. A subsequent replay must be a
    /// true no-op (or at worst rewrite identical bytes).
    ///
    /// We drive the writer's full `compact_community` pipeline, then
    /// reopen and replay twice to prove convergence is idempotent across
    /// arbitrarily many restarts.
    #[test]
    fn replay_compact_community_post_apply_is_idempotent() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        let cid = 7u32;
        let old_slots = [1u32, 2, 3];

        // Run the full writer flow. The new slots are chosen by
        // `compact_community` itself (page-aligned at the current tail):
        // node_hw = 43 (we wrote slots 1..3 and 42), NODES_PER_PAGE rounds
        // up, so new_start = the first aligned slot ≥ 43.
        let new_start;
        {
            let w = seed_community(&sub_path, &old_slots, cid);
            // node_hw = max slot written + 1 = 43 ; edge_hw = 3.
            let result = w
                .compact_community(cid, 43, 3, /*order=*/ 8, /*max_degree=*/ 64)
                .unwrap();
            assert_eq!(result.community_id, cid);
            assert_eq!(result.relocations.len(), 3);
            new_start = result.relocations[0].1;
            // New slots are contiguous.
            assert_eq!(result.relocations[1].1, new_start + 1);
            assert_eq!(result.relocations[2].1, new_start + 2);
            w.commit().unwrap();
            w.wal().fsync().unwrap();
        }

        // The writer's internal sort is by (hilbert_key, old_slot) où le
        // hilbert_key tient compte du degré RÉEL (Step 7) — l'ordre des
        // membres n'est donc plus trivial. Les nouveaux slots sont contigus
        // et page-alignés (asserté plus haut) ; le helper d'assertion
        // reconstitue le mapping old→new PAR LABEL, robuste à l'ordre.
        let new_slots = [new_start, new_start + 1, new_start + 2];

        // First replay pass — mmap is already post-compaction.
        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats1 = replay_from(&sub, 0).unwrap();
        assert_eq!(stats1.decode_errors, 0);
        assert_post_compaction_state(&sub, &old_slots, &new_slots, cid);

        // Second replay pass — convergence must be stable.
        drop(sub);
        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats2 = replay_from(&sub, 0).unwrap();
        assert_eq!(stats2.decode_errors, 0);
        assert_eq!(
            stats1.applied, stats2.applied,
            "replay must apply the same number of records on every pass"
        );
        assert_post_compaction_state(&sub, &old_slots, &new_slots, cid);
    }

    // ── (2026-08-19) L AUDIT D ANCRES SOUS PropsZone v2 ──
    //
    // Les quatre tests qui suivent verrouillent les DEUX moitiés de la
    // correction. Éteindre le faux positif ne vaut rien si l audit devient
    // muet : un audit qui ne détecte plus rien serait pire que le bruit
    // qu on lui reproche.

    /// Construit une base v2 saine : une chaîne de propriétés par nœud, donc
    /// des ancres qui sont des IDENTIFIANTS DE PAGE — ce que produit le
    /// chemin d écriture réel. Rend le chemin de la base et les ancres.
    fn base_v2_saine(dir: &std::path::Path, nb_noeuds: u32) -> (std::path::PathBuf, Vec<u32>) {
        use crate::page::{PropertyEntry, PropertyValue};
        let sub_path = dir.join("kb");
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let mut pz = crate::props_zone::PropsZone::open(&sub).unwrap();
        let ancres: Vec<u32> = (0..nb_noeuds)
            .map(|id| {
                let e = PropertyEntry::new(1, PropertyValue::I64(id as i64));
                pz.append_entry_node(id, None, &e).unwrap()
            })
            .collect();
        pz.flush().unwrap();
        drop(pz);

        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        for (id, page) in ancres.iter().copied().enumerate() {
            let mut rec = n(id as u32, 1);
            rec.first_prop_off = crate::props_zone::encode_page_id(page);
            w.write_node(id as u32, rec).unwrap();
        }
        w.commit().unwrap();
        w.msync_zones().unwrap();
        (sub_path, ancres)
    }

    /// LE FAUX POSITIF MESURÉ : sur la base réelle, `dangling` est passé de 0
    /// à 49 850 sans qu un octet de propriété ait bougé — il a suffi qu une
    /// écriture recrée `substrate.props`, dont la longueur servait de borne.
    /// On rejoue exactement cet enchaînement en miniature.
    #[test]
    fn audit_v2_base_saine_ne_declare_aucune_ancre_pendante() {
        let dir = tempdir().unwrap();
        let (sub_path, ancres) = base_v2_saine(dir.path(), 32);

        // Le fichier hérité renaît, court (16 octets). Sous l ancien critère
        // toute ancre >= 16 devenait « pendante » : ici 17 des 32 nœuds.
        std::fs::write(sub_path.join(crate::file::zone::PROPS), [0u8; 16]).unwrap();
        assert!(
            ancres.iter().filter(|p| **p as u64 >= 16).count() > 0,
            "le scénario doit vraiment exercer l ancien faux positif"
        );

        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(
            stats.dangling_prop_anchors, 0,
            "les ancres sont des index de page tous alloués : rien ne pend"
        );
        assert_eq!(
            stats.label_only_nodes, 0,
            "chaque nœud a une chaîne : aucune coquille"
        );
    }

    /// LE FAUX POSITIF QUI CRIAIT SUR TOUS LES STORES (2026-09-16).
    ///
    /// Mesuré : sur un store v3 **vide** — `count(n) = 0` — l'audit annonçait
    /// `label_only = 1` et « properties written after the last flush were lost »,
    /// alors qu'aucune propriété n'était perdue (la relecture rendait chacune des
    /// propriétés écrites). Il criait donc sur 100 % des stores, et noyait la
    /// signature de coquille qu'il existe pour porter — la panne exacte que ses
    /// deux compteurs devaient rendre VISIBLE.
    ///
    /// Cause : une zone v3 grave `NODES_ZONE_V3_MAGIC` dans le `label_bitset` de
    /// son SLOT 0 (c'est son marqueur de stride) en laissant ses flags à zéro. Ce
    /// record n'est vivant pour aucun chemin de lecture, mais `label_bitset != 0`
    /// suffisait à l'ancien test pour le compter comme une coquille.
    ///
    /// POURQUOI LE TEST v2 EXISTANT NE POUVAIT PAS L'ATTRAPER : il construit une
    /// zone **v2**, où le slot 0 est VIDE (`label_bitset == 0`) — donc jamais
    /// compté. Il fallait rejouer un store v3 RÉEL, celui que `Self::create`
    /// produit et que servent toutes les bases du dépôt.
    #[test]
    fn audit_v3_store_neuf_ne_declare_aucune_coquille() {
        let dir = tempdir().unwrap();
        let sub = SubstrateFile::create(dir.path().join("store")).unwrap();

        // ── GARDE DE NON-VACUITÉ ───────────────────────────────────────────
        // Le test ne prouve rien si la sentinelle n'a pas la FORME qui trompait
        // l'audit : magie non nulle dans le label_bitset, ancre nulle, et pas
        // vivante. Si sa forme change un jour, c'est ici qu'on l'apprend — au
        // lieu de croire que le défaut a disparu tout seul.
        {
            let zf = sub.open_zone(Zone::Nodes).unwrap();
            let buf = zf.as_slice();
            assert!(
                buf.len() >= NodeRecord::SIZE,
                "la zone nodes doit porter au moins son slot 0, soit {} octets",
                NodeRecord::SIZE
            );
            let slot0: &NodeRecord = bytemuck::from_bytes(&buf[..NodeRecord::SIZE]);
            assert_eq!(
                slot0.label_bitset,
                crate::record::NODES_ZONE_V3_MAGIC,
                "le slot 0 d'une zone v3 porte le marqueur de stride dans son label_bitset"
            );
            assert_ne!(
                slot0.label_bitset, 0,
                "c'est bien ce non-zéro qui trompait l'audit"
            );
            assert_eq!(
                slot0.first_prop_off.to_u64(),
                0,
                "la sentinelle ne porte aucune propriété"
            );
            assert!(
                !slot0.is_live(),
                "et elle n'est vivante pour aucun chemin de lecture"
            );
        }

        // ── L'ASSERTION QUI COMPTE ─────────────────────────────────────────
        let sub = SubstrateFile::open(dir.path().join("store")).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(
            stats.label_only_nodes, 0,
            "un store NEUF ne peut pas porter de coquille : la seule chose que \
             l'audit voyait était la sentinelle de sa propre zone"
        );
        assert_eq!(
            stats.dangling_prop_anchors, 0,
            "et rien ne pend dans une zone neuve"
        );
    }

    /// LE VRAI POSITIF QUI COMPTE : la signature de coquille du 25/07. Le WAL
    /// ne porte pour `NodeInsert` que le `label_bitset` ; un rejeu sur une
    /// zone nodes perdue recrée donc des nœuds À LABELS SANS ANCRE. L audit
    /// doit continuer à les compter — sur une base v2 comme ailleurs.
    #[test]
    fn audit_v2_signale_toujours_la_signature_de_coquille() {
        let dir = tempdir().unwrap();
        let (sub_path, _) = base_v2_saine(dir.path(), 32);

        // La zone nodes n avait pas atteint le disque au moment du kill.
        std::fs::write(sub_path.join(crate::file::zone::NODES), []).unwrap();

        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(
            stats.label_only_nodes, 32,
            "32 nœuds à labels sans propriétés atteignables — l audit doit crier"
        );
        assert_eq!(stats.dangling_prop_anchors, 0, "aucune ancre non nulle ici");
    }

    /// LES DEUX FORMES DE TÊTE PENDANTE MESURÉES EN 89f5ca5e, sous v2 :
    /// l index au-delà du fichier (1 501 040 refus sur `messages`) et l index
    /// qui tombe dans le rembourrage jamais écrit, en magic 0x0 (112 280).
    /// Les deux sont hors de `1..frontière` : un seul test les attrape.
    #[test]
    fn audit_v2_signale_les_ancres_hors_zone_allouee() {
        let dir = tempdir().unwrap();
        let (sub_path, ancres) = base_v2_saine(dir.path(), 4);
        let frontiere = *ancres.iter().max().unwrap() + 1;

        // Du rembourrage : le fichier s étend bien au-delà des pages écrites.
        {
            let sub = SubstrateFile::open(&sub_path).unwrap();
            let mut zf = sub
                .open_named_zone(crate::props_zone::PROPS_V2_FILENAME)
                .unwrap();
            zf.grow_to(64 * crate::page::PAGE_SIZE as u64).unwrap();
            zf.msync().unwrap();
        }

        {
            let sub = SubstrateFile::open(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            // Nœud 10 : index dans le rembourrage — page jamais écrite.
            let mut coquille_rembourrage = n(10, 1);
            coquille_rembourrage.first_prop_off = crate::props_zone::encode_page_id(40);
            w.write_node(10, coquille_rembourrage).unwrap();
            // Nœud 11 : index au-delà du fichier entier.
            let mut coquille_hors_fichier = n(11, 1);
            coquille_hors_fichier.first_prop_off = crate::props_zone::encode_page_id(1_000_000);
            w.write_node(11, coquille_hors_fichier).unwrap();
            w.commit().unwrap();
            w.msync_zones().unwrap();
        }

        assert!(frontiere < 40, "le rembourrage doit être hors zone allouée");
        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(
            stats.dangling_prop_anchors, 2,
            "les deux ancres tombent hors de 1..{frontiere}"
        );
        assert_eq!(
            stats.label_only_nodes, 0,
            "les 4 nœuds sains gardent leur chaîne"
        );
    }

    /// L autre régime doit rester intact : ~1 528 bases n ont pas migré, et
    /// chez elles l ancre EST un décalage en octets dans `substrate.props`.
    #[test]
    fn audit_herite_conserve_le_critere_en_octets() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            let mut sain = n(0, 1);
            sain.first_prop_off = U48::from_u64(100);
            w.write_node(0, sain).unwrap();
            let mut pendant = n(1, 1);
            pendant.first_prop_off = U48::from_u64(999_999);
            w.write_node(1, pendant).unwrap();
            w.commit().unwrap();
            w.msync_zones().unwrap();
        }
        // Aucun `substrate.props.v2` ici : régime hérité.
        std::fs::write(sub_path.join(crate::file::zone::PROPS), vec![0u8; 4096]).unwrap();
        assert!(
            !sub_path.join(crate::props_zone::PROPS_V2_FILENAME).exists(),
            "ce test doit rester sur le chemin hérité"
        );

        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(
            stats.dangling_prop_anchors, 1,
            "999 999 dépasse les 4 096 octets durables, 100 non"
        );
    }

    /// ── PERF #3 : l'audit post-rejeu est O(fenêtre), pas O(zone) ──
    ///
    /// Preuve ROUGE → VERT du correctif : un store de `base` slots durables
    /// (flush + checkpoint, WAL tronqué) puis une fenêtre de `fenetre` nœuds
    /// post-checkpoint. Le rejeu n'écrit que la fenêtre : l'audit doit balayer
    /// exactement `fenetre` slots, pas `base + fenetre`. Avant le correctif, le
    /// compteur `audit_slots_scanned` vaudrait `base + fenetre` (la boucle
    /// `for i in 0..n` balayait toute la zone) et ce test échouerait — c'est la
    /// régression O(N) mesurée (169 ms / 5 M slots, indépendant du nombre de
    /// records rejoués).
    ///
    /// Il verrouille AUSSI la sémantique du bornage : les nœuds de base portent
    /// une ancre NON NULLE (nœud « sans propriétés » légal et durable), les
    /// nœuds de fenêtre une ancre nulle (signature de coquille du crash). Seuls
    /// les `fenetre` nœuds doivent être comptés en coquilles — jamais `base`.
    #[test]
    fn audit_post_rejeu_borne_a_la_fenetre_rejouee() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        let base: u32 = 100_000;
        let fenetre: u32 = 10_000;

        // Base durable : `base` nœuds avec une ancre NON NULLE (nœud « sans
        // propriétés » légal, pas une coquille), flush puis checkpoint.
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::Group).unwrap();
            for i in 0..base {
                let mut rec = n(i, 1);
                rec.first_prop_off = U48::from_u64(1);
                w.write_node(i, rec).unwrap();
            }
            w.commit().unwrap();
            let wal = w.wal();
            drop(w);
            let mut sub2 = SubstrateFile::open(&sub_path).unwrap();
            crate::checkpoint(&mut sub2, &wal).unwrap();
            drop(sub2);
        }

        // Fenêtre post-checkpoint : `fenetre` nœuds à ancre NULLE, puis
        // « crash » sans flush des zones — le WAL fait foi.
        {
            let sub = SubstrateFile::open(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::Group).unwrap();
            for i in 0..fenetre {
                w.write_node(base + i, n(base + i, 1)).unwrap();
            }
            w.commit().unwrap();
            w.fsync_wal().unwrap();
            drop(w);
        }

        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(stats.decode_errors, 0);

        // ── LE CŒUR DU CORRECTIF ──
        // L'audit a balayé exactement la fenêtre rejouée, pas la zone entière.
        assert_eq!(
            stats.audit_slots_scanned, fenetre as u64,
            "l'audit doit balayer la fenêtre rejouée ({fenetre} slots), pas la zone entière"
        );

        // ── ET LA SÉMANTIQUE EST PRÉSERVÉE ──
        // Seuls les nœuds de la fenêtre (ancre nulle) sont des coquilles de CE
        // crash ; les nœuds de base (ancre durable) n'en sont pas.
        assert_eq!(
            stats.label_only_nodes, fenetre as u64,
            "seuls les {fenetre} nœuds de la fenêtre portent la signature de coquille"
        );
        assert_eq!(stats.dangling_prop_anchors, 0, "aucune ancre non nulle ici");
    }
}
