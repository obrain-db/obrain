//! UN OPCODE WAL INCONNU REFUSE L'OUVERTURE — jamais lu comme vide ni partiel.
//!
//! Garantie v0.0.1 : un binaire v0.0.x qui rencontre dans `substrate.wal` une
//! frame COMPLÈTE, à CRC valide, dont le `kind` lui est inconnu, refuse
//! d'ouvrir la base avec une erreur qui NOMME l'opcode, et ne modifie rien.
//! Le CRC couvre l'octet `kind` : une telle frame ne peut venir que d'un
//! binaire PLUS RÉCENT, et tout ce qui la suit est illisible pour celui-ci.
//! L'ouvrir quand même, c'est rejouer un préfixe, poser des tombstones sur
//! un rejeu tronqué, puis TRONQUER le WAL au premier checkpoint : perte
//! définitive des frames postérieures.
//!
//! Ce qui RESTE une frontière de crash (ouverture `Ok`, frames antérieures
//! rejouées) : une queue déchirée — qu'elle s'arrête AVANT l'octet `kind`
//! ou au milieu du payload d'une frame dont le `kind` est inconnu. Sans CRC
//! valide, rien ne distingue cette queue d'une écriture en vol.
//!
//! Framing (voir `wal.rs`) : 28 octets d'en-tête — longueur u32 LE, kind u8,
//! flags u8, réservé u16 = 0, lsn u64 LE, timestamp i64 LE, crc32 u32 LE
//! calculé sur les 24 premiers octets puis le payload.

use obrain_common::types::Value;
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::{SubstrateError, SubstrateStore};
use std::path::Path;

/// Opcode qu'aucun binaire v0.0.x ne connaît : 0x7E, hors de toute plage
/// attribuée (0x01–0x58 connus ; 0x59 est RÉSERVÉ par le chantier MP-4 pour
/// `TxOuverte`, et 0xF0/0xFE/0xFF sont pris). Ne pas rapprocher de 0x58.
const OPCODE_FUTUR: u8 = 0x7E;

/// Forge une frame complète au framing de `wal.rs`, CRC valide.
fn forger_frame(kind: u8, lsn: u64, payload: &[u8]) -> Vec<u8> {
    let mut entete = Vec::with_capacity(24);
    entete.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    entete.push(kind);
    entete.push(0); // flags
    entete.extend_from_slice(&[0, 0]); // réservé
    entete.extend_from_slice(&lsn.to_le_bytes());
    entete.extend_from_slice(&1_700_000_000_000_000i64.to_le_bytes());
    let mut h = crc32fast::Hasher::new();
    h.update(&entete);
    h.update(payload);
    let crc = h.finalize();
    let mut frame = entete;
    frame.extend_from_slice(&crc.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn frame_future() -> Vec<u8> {
    forger_frame(OPCODE_FUTUR, 1_000_000, b"payload d'un binaire plus recent")
}

/// Base peuplée puis fermée : 5 nœuds checkpointés, puis 4 nœuds écrits APRÈS
/// le dernier checkpoint — le WAL porte donc des frames que seul le rejeu
/// restitue.
fn base_peuplee(dir: &Path) {
    let store = SubstrateStore::create(dir).expect("creation");
    store.definir_seuil_checkpoint(0);
    for i in 0..5i64 {
        let id = store.create_node(&["Avant"]);
        store.set_node_property(id, "i", Value::Int64(i));
    }
    store.checkpoint().expect("checkpoint");
    for i in 0..4i64 {
        let id = store.create_node(&["Apres"]);
        store.set_node_property(id, "i", Value::Int64(i));
    }
    drop(store);
}

fn lire(dir: &Path, nom: &str) -> Vec<u8> {
    std::fs::read(dir.join(nom)).unwrap_or_else(|e| panic!("{nom} illisible : {e}"))
}

fn ajouter_au_wal(dir: &Path, octets: &[u8]) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join("substrate.wal"))
        .expect("substrate.wal");
    f.write_all(octets).expect("append");
    f.sync_all().expect("sync");
}

fn compter(store: &SubstrateStore) -> (usize, usize) {
    (
        store.nodes_by_label("Avant").len(),
        store.nodes_by_label("Apres").len(),
    )
}

#[test]
fn un_opcode_inconnu_refuse_l_ouverture_en_le_nommant_et_laisse_le_wal_intact() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path();
    base_peuplee(dir);
    let wal_avant = lire(dir, "substrate.wal");
    assert!(!wal_avant.is_empty(), "le WAL doit porter des frames");
    let meta_avant = lire(dir, "substrate.meta");
    let nodes_avant = lire(dir, "substrate.nodes");

    ajouter_au_wal(dir, &frame_future());
    let wal_forge = lire(dir, "substrate.wal");
    let offset_attendu = wal_avant.len() as u64;

    // ── Ouverture inscriptible : REFUS, l'opcode est nommé ──────────────
    let err = SubstrateStore::open(dir)
        .err()
        .expect("un opcode inconnu doit REFUSER l'ouverture, pas ouvrir une base partielle");
    assert!(
        matches!(
            err,
            SubstrateError::WalOpcodeInconnu { kind: OPCODE_FUTUR, offset } if offset == offset_attendu
        ),
        "variante attendue WalOpcodeInconnu {{ kind: 0x7E, offset: {offset_attendu} }}, reçu : {err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains("0x7E") && message.contains(&offset_attendu.to_string()),
        "le message doit nommer l'opcode et l'offset : {message}"
    );

    // ── Ouverture lecteur : même refus ──────────────────────────────────
    let err_ro = SubstrateStore::open_read_only(dir)
        .err()
        .expect("l'ouverture en lecture seule doit refuser elle aussi");
    assert!(
        matches!(
            err_ro,
            SubstrateError::WalOpcodeInconnu {
                kind: OPCODE_FUTUR,
                ..
            }
        ),
        "lecture seule : {err_ro:?}"
    );

    // ── Rien n'a été modifié ────────────────────────────────────────────
    assert_eq!(
        lire(dir, "substrate.wal"),
        wal_forge,
        "le WAL doit rester intact (octets)"
    );
    assert_eq!(
        lire(dir, "substrate.meta"),
        meta_avant,
        "substrate.meta ne doit pas bouger"
    );
    assert_eq!(
        lire(dir, "substrate.nodes"),
        nodes_avant,
        "la zone nœuds ne doit pas bouger"
    );

    // ── Preuve que le WAL était lisible et entier : sans la frame future,
    //    la base se rouvre avec TOUT son contenu, y compris l'après-checkpoint.
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join("substrate.wal"))
        .unwrap();
    f.set_len(offset_attendu).unwrap();
    f.sync_all().unwrap();
    let store = SubstrateStore::open(dir).expect("sans la frame future, la base s'ouvre");
    assert_eq!(
        compter(&store),
        (5, 4),
        "aucune frame n'a été perdue ni tronquée"
    );
}

/// Queue déchirée AVANT l'octet `kind` (3 octets) : moins qu'un en-tête, ce
/// n'est pas une frame — frontière de crash, ouverture `Ok`.
#[test]
fn une_queue_dechiree_avant_le_kind_reste_une_frontiere_de_crash() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path();
    base_peuplee(dir);
    ajouter_au_wal(dir, &frame_future()[..3]);

    let store = SubstrateStore::open(dir).expect("une queue déchirée s'ouvre");
    assert_eq!(
        compter(&store),
        (5, 4),
        "les frames antérieures sont rejouées"
    );
}

/// Queue déchirée APRÈS l'octet `kind` (en-tête complet, payload tronqué) :
/// le CRC n'est pas vérifiable, rien ne distingue cette queue d'une écriture
/// en vol — frontière de crash, ouverture `Ok`.
#[test]
fn une_frame_inconnue_tronquee_apres_le_kind_reste_une_frontiere_de_crash() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path();
    base_peuplee(dir);
    let frame = frame_future();
    assert!(frame.len() > 28 + 8);
    ajouter_au_wal(dir, &frame[..28 + 8]);

    let store = SubstrateStore::open(dir).expect("une frame tronquée s'ouvre");
    assert_eq!(
        compter(&store),
        (5, 4),
        "les frames antérieures sont rejouées"
    );
}
