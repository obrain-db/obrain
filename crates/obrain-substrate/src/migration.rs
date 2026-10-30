//! Migration versionnee du format substrat (jalon 0, RFC principal-natif-v2 §3).
//!
//! ## Le mecanisme
//!
//! A l'ouverture EXCLUSIVE d'un store (`SubstrateFile::open`), si la version
//! du header meta est INFERIEURE a [`SUBSTRATE_FORMAT_VERSION`], la chaine de
//! migrations de [`table`] s'execute etape par etape jusqu'a la version du
//! binaire. Chaque etape :
//!
//!   1. recoit le REPERTOIRE du store et transforme les zones — elle doit
//!      etre IDEMPOTENTE (rejouable apres une panne a n'importe quel point)
//!      et ne JAMAIS toucher la version du header ;
//!   2. n'est validee que par [`ecrire_version_meta`], appele par la machine
//!      APRES le retour Ok de l'etape — c'est le POINT DE COMMIT (write +
//!      fsync du meta). Un kill a n'importe quel instant laisse soit un store
//!      a l'ancienne version (etape a rejouer), soit un store a la nouvelle
//!      version (etape terminee) — jamais un etat mixte revendique.
//!
//! Une version STRICTEMENT SUPERIEURE au binaire, ou une version ancienne
//! sans etape dans la table, est refusee par [`SubstrateError::UnsupportedVersion`]
//! — le refus propre historique (`validate_header`) reste la regle pour toute
//! ouverture NON exclusive (read-only, shared-write) : on ne migre jamais
//! sous un verrou partage.
//!
//! ## Le stride de la zone nodes (jalon 2)
//!
//! Depuis la v3, le layout de la zone nodes a DEUX strides possibles :
//! 32 octets (v1/v2) et 40 octets (v3). La version meta ne suffit pas a une
//! etape pour connaitre le stride : un kill entre le `rename` de la
//! reecriture 2→3 et le bump laisse une zone DEJA 40 o sous une version qui
//! dit encore 2 — rejouer la reecriture en lisant du 32 corromprait tout.
//! La zone v3 se decrit donc elle-meme : son SLOT 0 (reserve depuis
//! toujours, les compteurs demarrent a 1) porte
//! [`NODES_ZONE_V3_MAGIC`] dans son `label_bitset`. Toute etape qui touche
//! la zone nodes teste ce marqueur ([`zone_nodes_est_v3`]) avant de choisir
//! son stride. Un store 32 o reel a son slot 0 INTEGRALEMENT nul (aucun
//! writer n'y a jamais ecrit) — la collision est impossible en pratique.
//!
//! ## Surete WAL
//!
//! La machine tourne AVANT toute construction de `SubstrateStore`, donc avant
//! les passes de fusion WAL de l'ouverture (`merge_wal_inline_props`,
//! `merge_wal_external_edges` — qui ne touchent que props et aretes) ;
//! `replay_from` n'a aucun appelant en production (constat RFC v2 §0).
//!
//! La reecriture 2→3 CHANGE le layout de la zone nodes sans drainer le WAL,
//! et c'est VERIFIE sur : aucune variante de `WalPayload` ne porte d'offset
//! d'OCTETS dans la zone nodes. Les trames noeud (`NodeInsert`,
//! `NodeUpdate`, `NodeInsertV3`, `NodeUpdateV3`, `NodePropHeadUpdate`,
//! `NodeDelete`, ...) sont adressees par SLOT (`node_id`) et portent des
//! CHAMPS, pas des octets de record ; l'offset d'ecriture est recalcule au
//! rejeu avec le `NodeRecord::SIZE` du binaire courant. Les U48 qu'elles
//! transportent (`first_prop_off`) pointent dans la zone PROPS, dont le
//! layout ne bouge pas ; les offsets d'aretes pointent dans la zone EDGES,
//! inchangee. Un WAL ecrit avant la migration se rejoue donc correctement
//! sur la zone 40 o — la doctrine « drainer puis tronquer » du RFC §3 reste
//! la regle pour toute zone dont les trames porteraient des offsets
//! d'octets, ce qui n'est le cas d'aucune aujourd'hui.
//!
//! ## La table
//!
//! * **1 → 2** (jalon 1, §4-J1) : pose `node_flags::ALLOCATED` sur tout
//!   record de noeud NON-DEFAUT de la zone nodes. Le critere est le
//!   predicat de padding 8-champs herite du v1 — c'est ICI, et nulle part
//!   ailleurs, qu'il a le droit de servir : une derniere fois, pour benir
//!   l'existant. Layout inchange (le stride est detecte, voir ci-dessus) ;
//!   idempotente (OR d'un bit) ; rejouable apres kill a tout instant.
//! * **2 → 3** (jalon 2, §4-J2) : reecrit la zone nodes 32 → 40 octets vers
//!   `substrate.nodes.v3.tmp` puis `rename` atomique. Chaque record recoit
//!   `tenant_id = 0` — public DECLARE : l'etape pose aussi
//!   `meta_flags::MONO_TENANT_PUBLIC` dans le header (RFC §5.2 : le public
//!   se declare, ne se subit pas). La sentinelle A5 (`u32::MAX`) n'est
//!   JAMAIS posee par cette etape — elle est reservee au re-tenanting par
//!   lots d'un exploitant multi-tenant (§5.1). Les trous restent des trous
//!   (tout-zero, sans ALLOCATED : invisibles), les tombstones restent des
//!   tombstones. Le choix nouvelle-zone-puis-rename (plutot qu'une
//!   reecriture in-place) est impose par la mecanique : l'expansion 32→40
//!   deplace TOUS les offsets de slots — in-place, un kill a mi-course
//!   laisserait une zone a deux strides indiscernables ; le rename est
//!   atomique (POSIX) et la reconstruction du tmp repart de zero a chaque
//!   tentative. Idempotente par le marqueur de slot 0 ; kill-sure : avant
//!   le rename c'est l'ancienne zone qui fait foi, apres c'est la nouvelle,
//!   et la version ne bumpe qu'en dernier.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::zone;
use crate::meta::{SUBSTRATE_FORMAT_VERSION, SUBSTRATE_MAGIC, meta_flags};
use crate::record::{NODES_ZONE_V3_MAGIC, NodeRecord, U48, node_flags};

/// Une etape de migration `de` → `vers` (avec `de < vers <= version binaire`).
///
/// `etape` recoit le repertoire du store. CONTRAT : idempotente, ne touche
/// pas la version meta (c'est le role de la machine), `fsync` ses propres
/// ecritures avant de rendre Ok.
pub struct Migration {
    pub de: u32,
    pub vers: u32,
    pub etape: fn(&Path) -> SubstrateResult<()>,
}

/// La table de production.
pub fn table() -> &'static [Migration] {
    &[
        Migration {
            de: 1,
            vers: 2,
            etape: migration_1_vers_2,
        },
        Migration {
            de: 2,
            vers: 3,
            etape: migration_2_vers_3,
        },
    ]
}

// ---------------------------------------------------------------------------
// Layout v2 GELE (32 octets) — la copie que les etapes lisent pour toujours
// ---------------------------------------------------------------------------

/// Le `NodeRecord` des formats 1 et 2, FIGE a 32 octets. Le type vivant
/// (`record::NodeRecord`) continuera de grossir ; les etapes de migration,
/// elles, lisent les zones anciennes avec CE layout-la, qui ne bougera
/// plus jamais. Champs identiques au v2, offsets identiques (flags a 30).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NoeudV2 {
    pub label_bitset: u64,
    pub first_edge_off: U48,
    pub first_prop_off: U48,
    pub community_id: u32,
    pub energy: u16,
    pub scar_util_affinity: u16,
    pub centrality_cached: u16,
    pub flags: u16,
}

impl NoeudV2 {
    pub const SIZE: usize = 32;
}

const _: () = assert!(core::mem::size_of::<NoeudV2>() == NoeudV2::SIZE);
const _: () = assert!(core::mem::offset_of!(NoeudV2, flags) == 30);
// Le tenant s'ajoute en QUEUE : tout champ v2 garde son offset dans le v3.
const _: () = assert!(core::mem::offset_of!(NodeRecord, flags) == 30);
const _: () = assert!(core::mem::offset_of!(NodeRecord, tenant_id) == 32);

/// La zone nodes de `dir` est-elle deja au stride v3 (40 o) ? Vrai si son
/// slot 0 porte le marqueur [`NODES_ZONE_V3_MAGIC`] (voir la doc de module).
pub fn zone_nodes_est_v3(dir: &Path) -> SubstrateResult<bool> {
    let chemin = dir.join(zone::NODES);
    if !chemin.exists() {
        return Ok(false);
    }
    let mut f = std::fs::File::open(&chemin)?;
    let mut tete = [0u8; 8];
    match f.read_exact(&mut tete) {
        Ok(()) => Ok(u64::from_le_bytes(tete) == NODES_ZONE_V3_MAGIC),
        // Zone plus courte que 8 octets (vide) : pas de marqueur.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------
// Etape 1 → 2 (jalon 1) : benediction ALLOCATED
// ---------------------------------------------------------------------------

/// Jalon 1 (RFC §4-J1) : benit tout record non-defaut avec `ALLOCATED`.
fn migration_1_vers_2(dir: &Path) -> SubstrateResult<()> {
    poser_allocated_zone_nodes(dir, None)
}

/// Le corps de la migration 1→2, borne pour les BANCS (`Some(k)` = ne
/// traiter que les slots < k — simuler une migration interrompue a
/// mi-course). Production : `None` = toute la zone.
///
/// Le stride est DETECTE (marqueur v3, voir doc de module) : un store
/// ancien reel est lu en 32 o ; une fixture de banc fabriquee en reculant
/// la version d'un store du binaire courant est lue en 40 o. La logique
/// (benir les records non-defaut) est la meme aux deux strides.
///
/// Ecrit UNIQUEMENT les 2 octets de `flags` des records a benir (ecriture
/// alignee, au pire UNE dechirure de 2 octets par kill — et l'etape est
/// rejouee tant que la version n'a pas bumpe), puis `fsync`.
#[doc(hidden)]
pub fn poser_allocated_zone_nodes(dir: &Path, borne_slots: Option<u32>) -> SubstrateResult<()> {
    const OFF_FLAGS: usize = 30; // identique aux deux strides (verifie plus haut)

    let chemin = dir.join(zone::NODES);
    if !chemin.exists() {
        return Ok(());
    }
    let stride = if zone_nodes_est_v3(dir)? {
        NodeRecord::SIZE
    } else {
        NoeudV2::SIZE
    };
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(&chemin)?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    let n_slots = (bytes.len() / stride) as u32;
    let borne = borne_slots.map_or(n_slots, |b| b.min(n_slots));
    for slot in 1..borne {
        let base = slot as usize * stride;
        // Les 32 premiers octets d'un record v3 SONT un NoeudV2 (le tenant
        // est en queue) : le predicat 8-champs se lit pareil aux deux strides.
        let rec: NoeudV2 = bytemuck::pod_read_unaligned(&bytes[base..base + NoeudV2::SIZE]);
        if rec.flags & node_flags::ALLOCATED != 0 || est_padding_v1(&rec) {
            continue;
        }
        let flags = rec.flags | node_flags::ALLOCATED;
        f.seek(SeekFrom::Start((base + OFF_FLAGS) as u64))?;
        f.write_all(&flags.to_le_bytes())?;
    }
    f.sync_all()?;
    Ok(())
}

/// L'heuristique de padding 8-champs du v1 — SEUL site autorise, derniere
/// utilisation (benediction de l'existant). Limite assumee et documentee :
/// un noeud v1 cree par `create_node_in_community(&[], 0)` sans aucune
/// autre ecriture est byte-pour-byte identique a un trou — il devient un
/// trou a la migration. C'etait deja un trou pour les detecteurs v1
/// (warden, fenetres 2D) ; le RFC l'assume (§4-J1).
fn est_padding_v1(rec: &NoeudV2) -> bool {
    rec.label_bitset == 0
        && rec.community_id == 0
        && rec.centrality_cached == 0
        && rec.energy == 0
        && rec.scar_util_affinity == 0
        && rec.flags == 0
        && rec.first_edge_off == U48::ZERO
        && rec.first_prop_off == U48::ZERO
}

// ---------------------------------------------------------------------------
// Etape 2 → 3 (jalon 2) : reecriture 32 → 40, tenant 0 declare public
// ---------------------------------------------------------------------------

/// Jalon 2 (RFC §4-J2) : reecrit la zone nodes en 40 o, tenant 0 partout,
/// et DECLARE le store mono-tenant public dans le header meta.
fn migration_2_vers_3(dir: &Path) -> SubstrateResult<()> {
    reecrire_zone_nodes_2_vers_3(dir, None)?;
    // La declaration (§5.2) : tenant 0 n'est un defaut legitime que declare.
    // OR idempotent, apres la bascule de zone — un kill entre les deux
    // rejoue l'etape (marqueur v3 -> reecriture no-op, drapeau repose).
    poser_drapeau_meta(dir, meta_flags::MONO_TENANT_PUBLIC)
}

/// Le corps de la reecriture 2→3, avec une pause optionnelle toutes les
/// 4096 conversions — pour les BANCS uniquement (l'enfant kill -9 doit
/// pouvoir etre tue EN PLEINE construction du fichier temporaire).
///
/// Deroulement (kill-sur, voir doc de module) :
///   1. marqueur v3 present -> no-op (zone deja reecrite, idempotence) ;
///   2. construction de `substrate.nodes.v3.tmp` depuis la zone 32 o :
///      slot 0 = record sentinelle (marqueur), slots 1.. copies champ a
///      champ + `tenant_id = 0` ; un slot tout-zero (trou) reste tout-zero ;
///   3. `fsync` du tmp, `rename` atomique sur `substrate.nodes`, `fsync`
///      du repertoire — la bascule ;
///   4. la version meta n'est bumpee que par la machine, apres.
#[doc(hidden)]
pub fn reecrire_zone_nodes_2_vers_3(
    dir: &Path,
    pause_par_tranche: Option<std::time::Duration>,
) -> SubstrateResult<()> {
    use crate::record::tenant;

    if zone_nodes_est_v3(dir)? {
        return Ok(());
    }
    let chemin = dir.join(zone::NODES);
    let ancien = if chemin.exists() {
        std::fs::read(&chemin)?
    } else {
        Vec::new()
    };
    // Le reste de division (< 32 octets) est de la marge de croissance
    // jamais ecrite (zeros) : abandonnee, la zone regrossira a l'ecriture.
    let n_slots = ancien.len() / NoeudV2::SIZE;

    // Construction EN FLUX du fichier temporaire (tranches de 4096 slots,
    // flush par tranche) : un kill a n'importe quel instant laisse un tmp
    // dechire et l'ancienne zone INTACTE — la relance repart de zero
    // (truncate). C'est aussi ce qui rend la pause de banc observable :
    // le tmp existe et grossit PENDANT la reecriture.
    let tmp = dir.join("substrate.nodes.v3.tmp");
    {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        let mut w = std::io::BufWriter::new(f);
        w.write_all(bytemuck::bytes_of(&NodeRecord::sentinelle_zone_v3()))?;
        for slot in 1..n_slots {
            let base = slot * NoeudV2::SIZE;
            let v2: NoeudV2 = bytemuck::pod_read_unaligned(&ancien[base..base + NoeudV2::SIZE]);
            let v3 = NodeRecord {
                label_bitset: v2.label_bitset,
                first_edge_off: v2.first_edge_off,
                first_prop_off: v2.first_prop_off,
                community_id: v2.community_id,
                energy: v2.energy,
                scar_util_affinity: v2.scar_util_affinity,
                centrality_cached: v2.centrality_cached,
                flags: v2.flags,
                // Public DECLARE (§5.2) — pour TOUT slot : un record vivant
                // devient public ; un trou (tout-zero, sans ALLOCATED) reste
                // tout-zero donc invisible ; un tombstone reste un tombstone
                // public. JAMAIS la sentinelle A5 ici.
                tenant_id: tenant::PUBLIC,
                _reserve: 0,
            };
            w.write_all(bytemuck::bytes_of(&v3))?;
            if slot % 4096 == 0 {
                w.flush()?;
                if let Some(pause) = pause_par_tranche {
                    std::thread::sleep(pause);
                }
            }
        }
        w.flush()?;
        w.get_ref().sync_all()?;
    }
    std::fs::rename(&tmp, &chemin)?;
    // fsync du repertoire : le rename lui-meme doit etre durable avant que
    // la machine ne bumpe la version.
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// OR un bit dans le champ `flags` du header meta (offset 12), write + fsync.
/// Idempotent.
fn poser_drapeau_meta(dir: &Path, bit: u32) -> SubstrateResult<()> {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join(zone::META))?;
    let mut tete = [0u8; 16];
    f.read_exact(&mut tete)?;
    if tete[..8] != SUBSTRATE_MAGIC {
        return Err(SubstrateError::BadMagic);
    }
    let flags = u32::from_le_bytes(tete[12..16].try_into().expect("4 octets"));
    if flags & bit == bit {
        return Ok(());
    }
    f.seek(SeekFrom::Start(12))?;
    f.write_all(&(flags | bit).to_le_bytes())?;
    f.sync_all()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// La machine
// ---------------------------------------------------------------------------

/// Execute la chaine de migrations de production sur `dir`.
/// No-op si le store est deja a la version courante.
pub fn migrer_repertoire(dir: &Path) -> SubstrateResult<()> {
    migrer_repertoire_avec_table(dir, table())
}

/// La machine, parametree par sa table — separee pour etre prouvable au
/// banc (tables simulees, etapes dechirees, kill -9) sans store ancien reel.
pub fn migrer_repertoire_avec_table(dir: &Path, table: &[Migration]) -> SubstrateResult<()> {
    loop {
        let v = lire_version_meta(dir)?;
        if v == SUBSTRATE_FORMAT_VERSION {
            return Ok(());
        }
        if v > SUBSTRATE_FORMAT_VERSION {
            // Store d'un binaire plus recent : refus propre, rien touche.
            return Err(SubstrateError::UnsupportedVersion(v, SUBSTRATE_FORMAT_VERSION));
        }
        let Some(m) = table.iter().find(|m| m.de == v) else {
            // Version ancienne sans chemin : refus propre, rien touche.
            return Err(SubstrateError::UnsupportedVersion(v, SUBSTRATE_FORMAT_VERSION));
        };
        debug_assert!(
            m.de < m.vers && m.vers <= SUBSTRATE_FORMAT_VERSION,
            "table de migration incoherente: {} -> {}",
            m.de,
            m.vers
        );
        (m.etape)(dir)?;
        // POINT DE COMMIT — la version ne bouge qu'apres le succes complet
        // de l'etape. Un kill avant cette ligne rejoue l'etape a la
        // prochaine ouverture ; un kill apres la considere terminee.
        ecrire_version_meta(dir, m.vers)?;
    }
}

/// Lit `format_version` dans le header du `substrate.meta` de `dir`.
pub fn lire_version_meta(dir: &Path) -> SubstrateResult<u32> {
    let mut f = std::fs::File::open(dir.join(zone::META))?;
    let mut tete = [0u8; 12];
    f.read_exact(&mut tete)?;
    if tete[..8] != SUBSTRATE_MAGIC {
        return Err(SubstrateError::BadMagic);
    }
    Ok(u32::from_le_bytes(tete[8..12].try_into().expect("4 octets")))
}

/// Ecrit `format_version` dans le header (write + fsync) — le point de
/// commit de la machine.
///
/// Public (cache) pour les BANCS uniquement : fabriquer un store « ancien »
/// de test. Aucun chemin de production ne doit reculer une version.
#[doc(hidden)]
pub fn ecrire_version_meta(dir: &Path, version: u32) -> SubstrateResult<()> {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join(zone::META))?;
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic)?;
    if magic != SUBSTRATE_MAGIC {
        return Err(SubstrateError::BadMagic);
    }
    f.seek(SeekFrom::Start(8))?;
    f.write_all(&version.to_le_bytes())?;
    f.sync_all()?;
    Ok(())
}
