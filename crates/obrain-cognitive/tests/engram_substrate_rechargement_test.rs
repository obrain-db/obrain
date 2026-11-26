//! Rechargement des engrammes depuis le SUBSTRATE (`load_from_substrate`) et
//! re-dérivation des signatures spectrales via `EngramManager::attach_substrate`.
//! Couverture #414 — périmètre A : persistance cognitive, chemin substrate.

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::sync::Arc;

use obrain_cognitive::InMemoryVectorIndex;
use obrain_cognitive::engram::{EngramId, EngramManager, EngramManagerConfig, EngramStore};
use obrain_common::types::NodeId;
use obrain_substrate::{SubstrateFile, SyncMode, Writer as SubstrateWriter};

fn make_writer() -> (Arc<SubstrateWriter>, tempfile::TempDir) {
    let td = tempfile::tempdir().unwrap();
    let sub = SubstrateFile::create(td.path().join("kb")).unwrap();
    let w = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    (Arc::new(w), td)
}

#[test]
fn load_from_substrate_recharge_apres_redemarrage() {
    let (writer, _td) = make_writer();

    // Session 1 : formation native substrate (membres + poids persistés).
    {
        let store = EngramStore::new(None);
        store.attach_substrate(writer.clone());
        let e1 = store.form(&[NodeId(1), NodeId(2)]).unwrap();
        let e2 = store.form(&[NodeId(3), NodeId(4)]).unwrap();
        assert_eq!(e1, EngramId(1));
        assert_eq!(e2, EngramId(2));
    } // store1 dropped.

    // Session 2 : réouverture du même fichier substrate.
    let store2 = EngramStore::new(None);
    store2.attach_substrate(writer.clone());
    let loaded = store2.load_from_substrate();
    assert_eq!(loaded.coherent, 2, "deux engrammes rechargés depuis le substrate");

    let e1 = store2.get(EngramId(1)).expect("engram 1 rechargé");
    assert_eq!(e1.node_ids(), vec![NodeId(1), NodeId(2)]);
    assert_eq!(
        store2.get(EngramId(2)).unwrap().node_ids(),
        vec![NodeId(3), NodeId(4)]
    );

    // Réconciliation : le prochain id est au-dessus du max chargé.
    assert_eq!(store2.next_id(), EngramId(3));
}

#[test]
fn load_from_substrate_sans_writer_retourne_zero() {
    let store = EngramStore::new(None);
    assert_eq!(store.load_from_substrate().coherent, 0);
}

#[test]
fn load_from_substrate_quarantaine_sur_poids_incoherents() {
    let (writer, _td) = make_writer();

    // État incohérent injecté directement : 2 membres, 1 seul poids.
    // (contourne `set_engram_state` qui refuse membres.len() != poids.len()).
    writer.set_engram_members(1, vec![10, 20]).unwrap();
    writer.set_engram_weights(1, vec![1.0]).unwrap();

    let store = EngramStore::new(None);
    store.attach_substrate(writer);
    let loaded = store.load_from_substrate();

    assert_eq!(loaded.coherent, 0, "l'engramme incohérent est mis en quarantaine");
    assert_eq!(
        loaded.rejected.len(),
        1,
        "un rejet d'incohérence membres/poids est signalé"
    );
    assert!(
        store.get(EngramId(1)).is_none(),
        "aucun poids n'est inventé pour un état incohérent"
    );
    // L'identifiant reste RÉSERVÉ (jamais réalloué après corruption).
    assert!(store.next_id().0 > 1, "l'id corrompu est réservé");
}

#[test]
fn load_from_substrate_reconcilie_le_compteur() {
    let (writer, _td) = make_writer();
    {
        let store = EngramStore::new(None);
        store.attach_substrate(writer.clone());
        store.form(&[NodeId(1)]).unwrap();
        store.form(&[NodeId(2)]).unwrap();
        store.form(&[NodeId(3)]).unwrap();
    }
    let store2 = EngramStore::new(None);
    store2.attach_substrate(writer);
    store2.load_from_substrate();
    assert_eq!(store2.next_id(), EngramId(4));
}

#[test]
fn attach_substrate_du_manager_rederive_les_signatures() {
    let (writer, _td) = make_writer();

    // Session 1 : formation native (persiste membres + poids).
    {
        let store = EngramStore::new(None);
        store.attach_substrate(writer.clone());
        store.form(&[NodeId(1), NodeId(2)]).unwrap();
        store.form(&[NodeId(3), NodeId(4)]).unwrap();
    }

    // Session 2 : le MANAGER réattache le même writer — il recharge depuis le
    // substrate puis re-dérive la projection spectrale de chaque ensemble.
    let mgr = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    mgr.attach_substrate(writer.clone());

    let engrams = mgr.store().list();
    assert_eq!(engrams.len(), 2);
    for e in &engrams {
        assert!(
            !e.spectral_signature.is_empty(),
            "signature spectrale re-dérivée pour {:?}",
            e.id
        );
    }
    // L'index vectoriel a été ré-ensemencé (dimension non nulle).
    assert!(
        mgr.vector_index_ref().dimensions() > 0,
        "index vectoriel ré-ensemencé après réouverture"
    );
}
