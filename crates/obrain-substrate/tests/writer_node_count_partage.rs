//! Régression : `bulk_sort_by_hilbert` écrivait `node_count` depuis la valeur
//! passée par l'appelant, sans relire la méta sur disque.
//!
//! En écriture partagée (`SubstrateFile::open_shared_write`), un pair peut avoir
//! agrandi le store entre-temps (ici : une compaction de communauté qui pousse
//! `node_count` à `new_node_hw`). Un écrivain resté à une vue périmée (`node_high_water`
//! petit) qui trie alors ramenait `node_count` à cette vue périmée, et les nœuds
//! du pair devenaient invisibles au décompte de la méta.
//!
//! Attendu : après le tri périmé, `node_count` vaut au moins ce que le pair a
//! écrit (ne jamais réduire). Sans correctif, ce test échoue.

use obrain_substrate::{
    NodeRecord, PackedScarUtilAff, SubstrateFile, U48, Writer, f32_to_q1_15, wal_io::SyncMode,
};

fn noeud(communaute: u32, centralite: u16) -> NodeRecord {
    NodeRecord {
        label_bitset: 1,
        first_edge_off: U48::default(),
        first_prop_off: U48::default(),
        community_id: communaute,
        energy: f32_to_q1_15(0.5),
        scar_util_affinity: PackedScarUtilAff::new(0, 0, 0, false).pack(),
        centrality_cached: centralite,
        flags: 0,
        tenant_id: 0,
        _reserve: 0,
    }
}

#[test]
fn bulk_sort_perime_ne_reduit_pas_node_count_du_pair() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");

    // Base initiale : 3 nœuds de la communauté 1, flushés.
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        for i in 0..3u32 {
            w.write_node(i, noeud(1, 1000 + i as u16)).unwrap();
        }
        w.commit().unwrap();
        w.vider_fenetre_de_rejeu().unwrap();
    }

    // Deux écrivains partagés sur le même store.
    let perime = Writer::new(
        SubstrateFile::open_shared_write(&path).unwrap(),
        SyncMode::EveryCommit,
    )
    .unwrap();
    let pair = Writer::new(
        SubstrateFile::open_shared_write(&path).unwrap(),
        SyncMode::EveryCommit,
    )
    .unwrap();

    // Le pair ajoute 8 nœuds (communauté 2) puis les compacte : son écriture
    // porte node_count à new_node_hw (plage fraîche page-alignée + 8).
    for i in 3..11u32 {
        pair.write_node(i, noeud(2, 2000 + i as u16)).unwrap();
    }
    pair.commit().unwrap();
    pair.vider_fenetre_de_rejeu().unwrap();
    let apres_pair = pair.compact_community(2, 11, 0, 4, 32).unwrap().new_node_hw;
    assert!(apres_pair >= 11, "scénario non probant : {apres_pair}");

    // L'écrivain périmé (il ne connaît que 3 slots) trie.
    perime.bulk_sort_by_hilbert(3, 0, 4, 32).unwrap();

    // Relecture à froid de la méta sur disque.
    drop(perime);
    drop(pair);
    let node_count = SubstrateFile::open(&path).unwrap().meta_header().node_count;
    assert!(
        node_count >= apres_pair as u64,
        "node_count ramené à une valeur périmée : {node_count} < {apres_pair} écrit par le pair"
    );
}
