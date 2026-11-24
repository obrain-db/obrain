//! obrain#350 — variantes du rejeu d'une transaction EN VOL : lecteur RO,
//! suppression d'une arête en vol, et ANCIENNES trames (`NodeInsert`,
//! `NodeUpdate`, `EdgeUpdate`) que les writers actuels n'émettent plus mais que
//! le rejeu doit toujours lire sans ressusciter l'entité d'une transaction
//! jamais scellée.

use std::path::Path;

use obrain_common::types::{EpochId, NodeId, TransactionId};
use obrain_core::graph::GraphStoreMut as _;
use obrain_substrate::replay::{replay_from_lecteur, replay_from_observe};
use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, SyncMode, WalWriter};
use obrain_substrate::{EdgeRecord, NodeRecord, SubstrateFile, SubstrateStore, Zone, replay_from};

const TX_MORTE: u64 = 91;

fn copier(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let cible = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copier(&e.path(), &cible);
        } else {
            std::fs::copy(e.path(), &cible).unwrap();
        }
    }
}

fn noeud_vivant(sub: &SubstrateFile, id: u32) -> bool {
    let z = sub.open_zone(Zone::Nodes).unwrap();
    let o = id as usize * NodeRecord::SIZE;
    if o + NodeRecord::SIZE > z.as_slice().len() {
        return false;
    }
    let r: &NodeRecord = bytemuck::from_bytes(&z.as_slice()[o..o + NodeRecord::SIZE]);
    r.is_live()
}

fn arete_vivante(sub: &SubstrateFile, id: u64) -> bool {
    let z = sub.open_zone(Zone::Edges).unwrap();
    let o = id as usize * EdgeRecord::SIZE;
    if o + EdgeRecord::SIZE > z.as_slice().len() {
        return false;
    }
    let r: &EdgeRecord = bytemuck::from_bytes(&z.as_slice()[o..o + EdgeRecord::SIZE]);
    !r.is_tombstoned()
}

fn rec(payload: WalPayload) -> WalRecord {
    WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload,
    }
}

/// Le lecteur RO n'écarte RIEN : l'écrivain peut être vivant et ne pas avoir
/// posé son `TxCommit`. (`replay_from_avec(.., false, None)`, `EnVol::new()`.)
#[test]
fn le_lecteur_ro_n_ecarte_pas_la_transaction_en_vol() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store");
    let image = td.path().join("image");

    let s = SubstrateStore::create(&dir).expect("créer");
    s.flush().expect("flush");
    let tx = TransactionId(TX_MORTE);
    let a = s.create_node_versioned(&["Vol"], EpochId::new(0), tx);
    let b = s.create_node_versioned(&["Vol"], EpochId::new(0), tx);
    let e = s.create_edge_versioned(a, b, "LIEN", EpochId::new(0), tx);
    copier(&dir, &image);
    drop(s);

    let sub = SubstrateFile::open(&image).expect("ouvrir");
    let st = replay_from_lecteur(&sub, 0).expect("rejeu lecteur");
    assert!(st.applied > 0, "le lecteur rejoue bien la fenêtre");
    assert_eq!(st.transactions_en_vol, 0, "le lecteur n'écarte aucune tx");
    assert_eq!(st.entites_en_vol_ecartees, 0);
    let NodeId(ia) = a;
    let NodeId(ib) = b;
    assert!(noeud_vivant(&sub, ia as u32), "nœud a intact côté lecteur");
    assert!(noeud_vivant(&sub, ib as u32), "nœud b intact côté lecteur");
    assert!(arete_vivante(&sub, e.0), "arête intacte côté lecteur");

    // Contraste : le rejeu ÉCRIVAIN, lui, écarte la même transaction.
    let st = replay_from(&sub, 0).expect("rejeu écrivain");
    assert_eq!(st.transactions_en_vol, 1);
    assert!(!noeud_vivant(&sub, ia as u32));
    assert!(!noeud_vivant(&sub, ib as u32));
    assert!(!arete_vivante(&sub, e.0));
}

/// Une arête créée puis SUPPRIMÉE dans la transaction n'est plus « en vol » :
/// l'automate la retire (`EdgeDelete`), seuls les deux nœuds restent écartés.
#[test]
fn une_arete_supprimee_n_est_plus_en_vol() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store");
    let image = td.path().join("image");

    let s = SubstrateStore::create(&dir).expect("créer");
    s.flush().expect("flush");
    let tx = TransactionId(TX_MORTE);
    let a = s.create_node_versioned(&["Vol"], EpochId::new(0), tx);
    let b = s.create_node_versioned(&["Vol"], EpochId::new(0), tx);
    let e = s.create_edge_versioned(a, b, "LIEN", EpochId::new(0), tx);
    assert!(s.delete_edge(e), "la suppression de l'arête doit réussir");
    copier(&dir, &image);
    drop(s);

    let sub = SubstrateFile::open(&image).expect("ouvrir");
    let st = replay_from(&sub, 0).expect("rejeu");
    assert_eq!(st.transactions_en_vol, 1, "les nœuds restent en vol");
    assert_eq!(
        st.entites_en_vol_ecartees, 2,
        "2 nœuds écartés ; l'arête supprimée n'est plus retenue (serait 3 sinon)"
    );
    assert!(
        !arete_vivante(&sub, e.0),
        "l'arête supprimée n'est pas vivante"
    );
    let NodeId(ia) = a;
    let NodeId(ib) = b;
    assert!(!noeud_vivant(&sub, ia as u32));
    assert!(!noeud_vivant(&sub, ib as u32));
}

/// Construit un store vierge dont le WAL ne contient que `records`.
fn store_avec_wal(td: &Path, records: Vec<WalRecord>) -> SubstrateFile {
    let dir = td.join("store");
    let sub = SubstrateFile::create(&dir).expect("créer");
    {
        let w = WalWriter::open(sub.wal_path(), SyncMode::Never, ANCRE_INITIALE).unwrap();
        for r in records {
            w.append(r).unwrap();
        }
    }
    sub
}

/// ANCIENNES trames `NodeInsert` et `NodeUpdate` réécrivant les flags d'un
/// nœud d'une transaction jamais scellée : il n'est JAMAIS vivant, à aucun
/// record, à aucun des deux rejeux.
#[test]
fn anciennes_trames_noeud_ne_ressuscitent_pas_une_entite_en_vol() {
    let td = tempfile::tempdir().unwrap();
    let n = 5u32;
    let sub = store_avec_wal(
        td.path(),
        vec![
            rec(WalPayload::TxNoeudEnAttente {
                tx: TX_MORTE,
                node_id: n,
            }),
            rec(WalPayload::NodeInsert {
                node_id: n,
                label_bitset: 1,
            }),
            rec(WalPayload::NodeUpdate {
                node_id: n,
                label_bitset: 1,
                energy: 100,
                scar_util_affinity: 0,
                centrality_cached: 0,
                flags: 0,
                community_id: 0,
            }),
        ],
    );
    for rejeu in 1..=2 {
        let mut vivants: Vec<String> = Vec::new();
        let mut pas = 0u32;
        let st = replay_from_observe(&sub, 0, &mut || {
            pas += 1;
            if noeud_vivant(&sub, n) {
                vivants.push(format!("rejeu {rejeu}, record #{pas}"));
            }
        })
        .expect("rejeu");
        assert_eq!(pas, 3, "les trois records sont appliqués");
        assert_eq!(st.transactions_en_vol, 1);
        assert_eq!(st.entites_en_vol_ecartees, 1);
        assert!(vivants.is_empty(), "nœud ressuscité : {vivants:?}");
        assert!(!noeud_vivant(&sub, n));
    }
}

/// ANCIENNE trame `EdgeUpdate` (flags à 0) sur une arête en vol.
#[test]
fn ancienne_trame_edge_update_ne_ressuscite_pas_une_arete_en_vol() {
    let td = tempfile::tempdir().unwrap();
    let e = 3u64;
    let sub = store_avec_wal(
        td.path(),
        vec![
            rec(WalPayload::TxAreteEnAttente {
                tx: TX_MORTE,
                edge_id: e,
            }),
            rec(WalPayload::EdgeInsert {
                edge_id: e,
                src: 1,
                dst: 2,
                edge_type: 1,
                weight_u16: 10,
            }),
            rec(WalPayload::EdgeUpdate {
                edge_id: e,
                weight_u16: 20,
                ricci_u8: 0,
                flags: 0,
                engram_tag: 0,
            }),
        ],
    );
    for rejeu in 1..=2 {
        let mut vivantes: Vec<String> = Vec::new();
        let mut pas = 0u32;
        let st = replay_from_observe(&sub, 0, &mut || {
            pas += 1;
            if arete_vivante(&sub, e) {
                vivantes.push(format!("rejeu {rejeu}, record #{pas}"));
            }
        })
        .expect("rejeu");
        assert_eq!(pas, 3);
        assert_eq!(st.transactions_en_vol, 1);
        assert_eq!(st.entites_en_vol_ecartees, 1);
        assert!(vivantes.is_empty(), "arête ressuscitée : {vivantes:?}");
        assert!(!arete_vivante(&sub, e));
    }
}
