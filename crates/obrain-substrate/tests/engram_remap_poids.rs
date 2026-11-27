//! Le remap physique des slots (`bulk_sort_by_hilbert` = HilbertRepermuteV2,
//! `compact_community` = CompactCommunityV2) ré-écrit les MEMBRES d'un
//! engramme pondéré ; il doit ré-attacher les POIDS liés (V4, empreinte
//! FNV-1a en tête du snapshot) sans quoi `read_engram_state` rend
//! `Err(EngramIncoherent)` et l'engramme est mis en quarantaine au rechargement.
//!
//! Ces tests exercent le chemin RÉEL (pas une mutation manuelle) :
//! écriture d'un engramme pondéré → remap → lecture cohérente → checkpoint /
//! rejeu / réouverture → révision → suppression. Vérifient membres logiques,
//! poids, identités de nœud, références d'arêtes, projection (bitset) et rappel
//! (hopfield_recall) tous préservés.

use obrain_substrate::replay::replay_from;
use obrain_substrate::{
    EdgeRecord, NodeRecord, PackedScarUtilAff, SubstrateFile, U48, Writer, f32_to_q1_15,
    file::Zone, wal_io::SyncMode,
};
use std::collections::HashMap;

fn sample_node(community: u32, centrality: u16) -> NodeRecord {
    NodeRecord {
        label_bitset: 1,
        first_edge_off: U48::default(),
        first_prop_off: U48::default(),
        community_id: community,
        energy: f32_to_q1_15(0.5),
        scar_util_affinity: PackedScarUtilAff::new(0, 0, 0, false).pack(),
        centrality_cached: centrality,
        flags: 0,
        tenant_id: 0,
        _reserve: 0,
    }
}

fn sample_edge(src: u32, dst: u32) -> EdgeRecord {
    EdgeRecord {
        src,
        dst,
        edge_type: 1,
        weight_u16: f32_to_q1_15(0.9),
        next_from: U48::default(),
        next_to: U48::default(),
        first_prop_off: U48::ZERO,
        ricci_u8: 128,
        flags: 0,
        engram_tag: 0,
        _pad: [0; 2],
    }
}

/// Reconstruit `old -> new` depuis la zone nœuds (source de vérité) en
/// identifiant chaque nœud par son marqueur `centrality_cached` (distinct).
fn old_to_new_from_nodes(w: &Writer, n: u32, centrality: &[u16]) -> Vec<u32> {
    let mut map = vec![u32::MAX; n as usize];
    for new_slot in 0..n {
        let rec = w
            .read_node(new_slot)
            .unwrap()
            .expect("nœud manquant après remap");
        let old_slot = centrality
            .iter()
            .position(|&c| c == rec.centrality_cached)
            .expect("centralité inconnue") as u32;
        map[old_slot as usize] = new_slot;
    }
    assert!(
        map.iter().all(|&x| x != u32::MAX),
        "permutation non bijective : {map:?}"
    );
    map
}

/// Pose les bitsets de l'engramme 1 sur chacun de ses membres (bit 1), pour
/// que le rappel ait une projection réelle à interroger.
fn poser_bitsets(w: &Writer, members: &[u32]) {
    for &m in members {
        w.add_engram_bit(m, 1).unwrap();
    }
}

#[test]
fn bulk_sort_preserve_les_poids_lies_de_l_engramme() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");

    // Disposition brouillée (3 communautés × 2 nœuds) : le tri Hilbert est
    // une VRAIE permutation (non identité) sur les slots [0, 6).
    let layout: Vec<u32> = vec![2, 0, 2, 1, 0, 1];
    let centrality: Vec<u16> = vec![1000, 2000, 3000, 500, 1500, 800];
    let n = layout.len() as u32;
    let members: Vec<u32> = vec![1, 2, 3, 4, 5];
    let weights: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4, 0.5];

    {
        let sub = SubstrateFile::create(&path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        for i in 0..n {
            w.write_node(i, sample_node(layout[i as usize], centrality[i as usize]))
                .unwrap();
        }
        // Deux arêtes pour exercer la référence src/dst.
        w.write_edge(0, sample_edge(1, 2)).unwrap();
        w.write_edge(1, sample_edge(3, 4)).unwrap();
        w.set_engram_state(1, members.clone(), weights.clone())
            .unwrap();
        poser_bitsets(&w, &members);
        w.commit().unwrap();

        // Barrière de durabilité : fenêtre de rejeu vide avant tout remap.
        w.vider_fenetre_de_rejeu().unwrap();
        w.bulk_sort_by_hilbert(n, 2, 4, 32).unwrap();

        // ── 1. Lecture COHÉRENTE : ni EngramIncoherent, ni poids altérés ──
        let (got_members, got_weights) = w.read_engram_state(1).unwrap().expect("engram disparu");
        assert_eq!(
            got_weights.as_deref(),
            Some(weights.as_slice()),
            "poids altérés par le remap"
        );
        let perm = w.derniere_permutation_remap().expect("permutation absente");
        let expected: Vec<u32> = members.iter().map(|&m| perm[m as usize]).collect();
        assert_eq!(got_members, expected, "membres non remappés");
        assert_ne!(got_members, members, "remap sans effet — test non probant");

        // ── 2. Identités de nœud préservées (marqueur au nouveau slot) ──
        let old_to_new = old_to_new_from_nodes(&w, n, &centrality);
        for (old, &new) in old_to_new.iter().enumerate() {
            let rec = w.read_node(new).unwrap().unwrap();
            assert_eq!(
                rec.centrality_cached, centrality[old],
                "identité du nœud {old} perdue"
            );
        }

        // ── 3. Références d'arêtes remappées ──
        let sub_arc = w.substrate();
        let sub = sub_arc.lock();
        let ez = sub.open_zone(Zone::Edges).unwrap();
        let post: &[EdgeRecord] = bytemuck::cast_slice(&ez.as_slice()[..2 * EdgeRecord::SIZE]);
        assert_eq!(post[0].src, old_to_new[1], "arête 0 src non remappée");
        assert_eq!(post[0].dst, old_to_new[2], "arête 0 dst non remappée");
        assert_eq!(post[1].src, old_to_new[3], "arête 1 src non remappée");
        assert_eq!(post[1].dst, old_to_new[4], "arête 1 dst non remappée");

        // ── 4. Projection (bitset) + rappel (hopfield_recall) préservés ──
        let recall = w.hopfield_recall(old_to_new[1], 10).unwrap();
        let mut got: Vec<u32> = recall.iter().map(|&(slot, _)| slot).collect();
        got.sort_unstable();
        let mut want = expected.clone();
        want.sort_unstable();
        assert_eq!(got, want, "rappel post-remap incohérent");
    }

    // ── 5. Réouverture + rejeu depuis 0 : état reconstruit identique ──
    {
        let sub = SubstrateFile::open(&path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert_eq!(stats.decode_errors, 0, "frame indécodable au rejeu");
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        let (got_members, got_weights) = w.read_engram_state(1).unwrap().expect("engram disparu");
        assert_eq!(got_weights.as_deref(), Some(weights.as_slice()));
        let old_to_new = old_to_new_from_nodes(&w, n, &centrality);
        let expected: Vec<u32> = members.iter().map(|&m| old_to_new[m as usize]).collect();
        assert_eq!(
            got_members, expected,
            "membres divergents après réouverture"
        );

        // ── 6. Révision ──
        let new_members = vec![1u32, 3, 5];
        let new_weights = vec![0.9f32, 0.8, 0.7];
        w.set_engram_state(1, new_members.clone(), new_weights.clone())
            .unwrap();
        let state = w.read_engram_state(1).unwrap().unwrap();
        assert_eq!(state.0, new_members);
        assert_eq!(state.1.as_deref(), Some(new_weights.as_slice()));

        // ── 7. Suppression ──
        w.set_engram_state(1, vec![], vec![]).unwrap();
        assert!(w.read_engram_state(1).unwrap().is_none());
    }
}

#[test]
fn compact_community_preserve_les_poids_lies_de_l_engramme() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let sub = SubstrateFile::create(&path).unwrap();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();

    // Slots 1..6 (le slot 0 reste vide), communautés 1 et 2 entrelacées.
    let layout: Vec<u32> = vec![1, 2, 1, 2, 1, 2]; // slot 1..6
    let centrality: Vec<u16> = vec![101, 201, 102, 202, 103, 203];
    for (i, (&cid, &c)) in layout.iter().zip(centrality.iter()).enumerate() {
        w.write_node((i + 1) as u32, sample_node(cid, c)).unwrap();
    }
    let members: Vec<u32> = vec![1, 2, 3, 5]; // 1,3,5 ∈ communauté 1 ; 2 ∈ communauté 2
    let weights: Vec<f32> = vec![0.11, 0.22, 0.33, 0.44];
    w.set_engram_state(1, members.clone(), weights.clone())
        .unwrap();
    poser_bitsets(&w, &members);
    w.commit().unwrap();

    w.vider_fenetre_de_rejeu().unwrap();
    let result = w.compact_community(1, 7, 0, 4, 32).unwrap();
    assert!(
        !result.relocations.is_empty(),
        "aucune relocation — non probant"
    );

    // Membres remappés via la liste de relocations (identité ailleurs).
    let old_to_new: HashMap<u32, u32> = result.relocations.iter().copied().collect();
    let expected: Vec<u32> = members
        .iter()
        .map(|&m| old_to_new.get(&m).copied().unwrap_or(m))
        .collect();

    let (got_members, got_weights) = w.read_engram_state(1).unwrap().expect("engram disparu");
    assert_eq!(
        got_weights.as_deref(),
        Some(weights.as_slice()),
        "poids altérés par la compaction"
    );
    assert_eq!(
        got_members, expected,
        "membres non remappés par la compaction"
    );
    assert_ne!(got_members, members, "compaction sans effet sur l'engramme");

    // Rappel cohérent après compaction (projection bitset remappée).
    let query = old_to_new.get(&1).copied().unwrap_or(1);
    let recall = w.hopfield_recall(query, 10).unwrap();
    let mut got: Vec<u32> = recall.iter().map(|&(slot, _)| slot).collect();
    got.sort_unstable();
    let mut want = expected.clone();
    want.sort_unstable();
    assert_eq!(got, want, "rappel post-compaction incohérent");
}
