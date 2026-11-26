//! Témoin indépendant — invariant de composabilité du remappage physique des
//! engrammes : « une optimisation physique ne doit pas provoquer une perte
//! cognitive ».
//!
//! Repro du défaut signalé : `Writer::compute_engram_remap_updates` /
//! `Writer::apply_engram_remap_updates` (crates/obrain-substrate/src/writer.rs)
//! remappent les MEMBRES d'un engramme (les slots passés par `old_to_new`) mais
//! ne re-lient PAS les POIDS. Or les poids sont liés à l'IDENTITÉ des membres :
//! la zone `ENGRAM_WEIGHTS` stocke `[fp_lo, fp_hi, w1..wN]` où `fp` est une
//! empreinte FNV-1a calculée sur les membres DANS L'ORDRE (voir
//! `EngramZone::set_weights_bound` / `interpret_engram_weights`). Un remap qui
//! mute les membres sans réécrire l'empreinte laisse une empreinte périmée ;
//! `read_engram_state` refuse alors la lecture avec
//! `EngramIncoherent::EmpreinteDivergente` — quarantaine injustifiée, donc
//! perte cognitive.
//!
//! Ce test est ROUGE par construction sur l'implémentation actuelle : il
//! documente les valeurs AVANT/APRÈS exactes et l'association logique perdue.
//! Il passe au VERT une fois que le correctif re-lie les poids (recalcul de
//! l'empreinte sur les membres remappés, poids inchangés dans l'ordre).
//!
//! NE MODIFIE PAS la production : ce fichier est un témoin de qualification.

use obrain_substrate::{
    f32_to_q1_15, file::Zone, wal_io::SyncMode, EdgeRecord, EngramIncoherence, NodeRecord,
    PackedScarUtilAff, SubstrateError, SubstrateFile, U48, Writer,
};

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

#[test]
fn remap_hilbert_non_trivial_ne_relie_pas_les_poids() {
    // ── 1. Fixture : layout brouillé (3 communautés × 4 nœuds), centralité
    //    unique par nœud (permet de reconstruire old→new après le tri).
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let sub = SubstrateFile::create(&path).unwrap();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();

    // Slot:        0  1  2  3  4  5  6  7  8  9 10 11
    // Community:   2  0  2  1  0  1  2  0  1  0  2  1
    let layout: Vec<u32> = vec![2, 0, 2, 1, 0, 1, 2, 0, 1, 0, 2, 1];
    let centrality: Vec<u16> = vec![
        1000, 2000, 3000, 500, 1500, 800, 2500, 100, 9000, 1200, 4000, 6000,
    ];
    let n = layout.len() as u32;
    for i in 0..n {
        w.write_node(i, sample_node(layout[i as usize], centrality[i as usize]))
            .unwrap();
    }
    let edges: Vec<(u32, u32)> = vec![
        (0, 2),
        (2, 6),
        (1, 4),
        (4, 9),
        (9, 7),
        (3, 5),
        (5, 8),
        (0, 1),
        (11, 10),
    ];
    for (i, &(src, dst)) in edges.iter().enumerate() {
        w.write_edge(i as u64, sample_edge(src, dst)).unwrap();
    }

    // ── 2. Engramme à poids NON uniformes et DISTINCTS (liés à l'identité
    //    des membres via `set_engram_state` → `set_weights_bound`).
    let engram_id: u16 = 1;
    // Membres = slots de nœuds, chacun identifiable par sa centralité unique :
    //   slot 1 → centrality 2000 (communauté 0)
    //   slot 2 → centrality 3000 (communauté 2)
    //   slot 3 → centrality 500  (communauté 1)
    //   slot 4 → centrality 1500 (communauté 0)
    let members: Vec<u32> = vec![1, 2, 3, 4];
    let weights: Vec<f32> = vec![0.1, 0.25, 0.5, 0.9];
    w.set_engram_state(engram_id, members.clone(), weights.clone())
        .unwrap();

    // ── 3. AVANT : l'état est cohérent (membres + poids liés).
    let before = w
        .read_engram_state(engram_id)
        .expect("lecture AVANT remap sans erreur")
        .expect("engramme présent AVANT remap");
    assert_eq!(before.0, members, "membres AVANT remap");
    assert_eq!(before.1, Some(weights.clone()), "poids AVANT remap");

    // ── 4. Remappage physique non trivial.
    w.commit().unwrap();
    // Chantier SlotRemappable — `Writer` NU, sans store : on vide nous-mêmes
    // la fenêtre de rejeu avant la repermutation (garde R9).
    w.vider_fenetre_de_rejeu().unwrap();
    w.bulk_sort_by_hilbert(n, edges.len() as u64, 4, 32).unwrap();

    // ── 5. Reconstruction de old→new (centralité unique par nœud).
    let sub_arc = w.substrate();
    let old_to_new: Vec<u32> = {
        let sub = sub_arc.lock();
        let nz = sub.open_zone(Zone::Nodes).unwrap();
        let post: &[NodeRecord] =
            bytemuck::cast_slice(&nz.as_slice()[..(n as usize) * NodeRecord::SIZE]);
        let mut m = vec![u32::MAX; n as usize];
        for (new_slot, rec) in post.iter().enumerate() {
            let old_slot = centrality
                .iter()
                .position(|&c| c == rec.centrality_cached)
                .expect("centralité post-sort non retrouvée") as u32;
            m[old_slot as usize] = new_slot as u32;
        }
        assert!(
            m.iter().all(|&x| x != u32::MAX),
            "permutation old→new non bijective"
        );
        m
    };

    let expected_remapped: Vec<u32> = members
        .iter()
        .map(|&m| old_to_new[m as usize])
        .collect();
    // Garde : le témoin ne prouve rien si la permutation est triviale sur les
    // membres choisis.
    assert_ne!(
        expected_remapped, members,
        "la permutation est triviale sur les membres {members:?} : choisir d'autres membres"
    );

    // ── 6. APRÈS — (a) les membres sont remappés (attendu, ça passe) …
    let got_members = w
        .engram_members(engram_id)
        .expect("lecture membres APRÈS remap sans erreur")
        .expect("membres présents APRÈS remap");
    assert_eq!(
        got_members, expected_remapped,
        "les membres doivent être remappés par old→new"
    );

    // ── 6. (b) … et l'invariant de composabilité exige que les poids restent
    //    liés aux MÊMES membres logiques (poids inchangés, dans le même ordre).
    match w.read_engram_state(engram_id) {
        Ok(Some((remapped, Some(w_after)))) => {
            assert_eq!(remapped, expected_remapped, "membres (état) remappés");
            assert_eq!(
                w_after, weights,
                "poids ré-associés à l'identique (positionnels) après remap"
            );
        }
        Ok(Some((_, None))) => panic!(
            "INVARIANT VIOLÉ : les poids ont disparu après remappage (Some(_, None)) \
             — perte cognitive"
        ),
        Ok(None) => panic!(
            "INVARIANT VIOLÉ : l'engramme {engram_id} a disparu après remappage — perte cognitive"
        ),
        Err(SubstrateError::EngramIncoherent {
            incoherence:
                EngramIncoherence::EmpreinteDivergente {
                    empreinte_stockee,
                    empreinte_attendue,
                },
            ..
        }) => panic!(
            "INVARIANT VIOLÉ — quarantaine injustifiée après remappage physique non trivial.\n\
             engram {engram_id} :\n\
             \x20 membres AVANT  : {members:?}\n\
             \x20 membres APRÈS  : {expected_remapped:?}\n\
             \x20 poids attendus : {weights:?} (inchangés, liés à l'identité des membres)\n\
             \x20 empreinte stockée : {empreinte_stockee:#018x} (calculée sur les ANCIENS membres)\n\
             \x20 empreinte attendue: {empreinte_attendue:#018x} (recalculée sur les membres remappés)\n\
             Association logique perdue : les poids ne sont plus reliables aux membres remappés — \
             `read_engram_state` refuse la lecture au lieu de ré-associer."
        ),
        Err(e) => panic!(
            "INVARIANT VIOLÉ — lecture refusée après remappage non trivial : {e}. \
             engram {engram_id} : membres {members:?} → {expected_remapped:?}, poids attendus {weights:?}"
        ),
    }
}
