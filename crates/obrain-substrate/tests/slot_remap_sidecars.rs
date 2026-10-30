//! Chantier SlotRemappable — « la donnée suit le nœud ».
//!
//! `bulk_sort_by_hilbert` / `compact_community` permutent les slots nœuds ;
//! ces tests vérifient que TOUS les side-cars slot-indexés suivent :
//! veccol (dont le dual-key `embedding_vec` déployé en production),
//! blobcol, degrés (totaux + typés), zones engram (membres + bitsets) et
//! table `slot_to_offset` du tier0.
//!
//! * T3 (`t3_veccol_suit_bulk_sort`) — écrit AVANT le fix et prouvé en
//!   ÉCHEC sur HEAD (les embeddings restaient aux anciens slots) ; il
//!   verrouille désormais l'enrôlement veccol.
//! * T1 (`t1_propriete_la_donnee_suit_le_noeud`) — property-test à
//!   plusieurs graines : store peuplé de tous les types de side-cars,
//!   permutation réelle (tri Hilbert sur communautés entrelacées),
//!   assertion PAR NŒUD sur chaque famille.
//! * T4 (`t4_golden_vector_top_k_avant_apres`) — parité du recall
//!   vectoriel avant/après repermutation. Les identités (NodeId = slot)
//!   changent par construction ; le tie-break `(sim desc, offset asc)`
//!   départage donc les ex æquo dans un ordre différent — c'est documenté
//!   et attendu : on asserte la parité des SCORES et l'auto-cohérence de
//!   chaque résultat (le score annoncé est bien le cosinus de l'embedding
//!   porté par le nœud rendu), pas l'égalité des offsets.
//! * compact_community — relocations PARTIELLES converties en permutation
//!   totale (identité + échanges) : les side-cars suivent aussi, y compris
//!   la croissance de hw (`ensure_room`, plage fraîche en queue).
//!
//! Identité de nœud à travers la permutation : le marqueur est
//! `centrality_cached` (u16 distinct par nœud), qui voyage DANS le
//! `NodeRecord`. Les propriétés scalaires du sidecar DashMap sont keyées
//! par NodeId et ne sont PAS remappées (hors périmètre, documenté au
//! rapport) — on ne s'en sert donc jamais comme marqueur.

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use obrain_substrate::blob_column::{BlobColSpec, BlobColumnReader};
use obrain_substrate::degree_column::DegreeColumn;
use obrain_substrate::file::{SubstrateFile, Zone};
use obrain_substrate::tier_persist::{self, TierMagic};
use obrain_substrate::tiers::Tier0;
use obrain_substrate::vec_column::{VecColSpec, VecColumnReader};
use std::collections::HashMap;

const DIM: usize = 16;

fn vec_for(marker: u64) -> Vec<f32> {
    let mut x = marker.wrapping_mul(2654435761).wrapping_add(7);
    (0..DIM)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((x >> 33) as f32 / (1u64 << 31) as f32) - 1.0
        })
        .collect()
}

fn body_for(marker: u64) -> String {
    // > 256 octets pour router en blobcol.
    format!("corps-{marker}-").repeat(40)
}

fn cosine(q: &[f32], v: &[f32]) -> f32 {
    let (mut dot, mut qn, mut vn) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..v.len() {
        dot += q[i] * v[i];
        qn += q[i] * q[i];
        vn += v[i] * v[i];
    }
    dot / (qn.sqrt() * vn.sqrt())
}

/// Peuple un store avec `n` nœuds aux communautés entrelacées et TOUTES
/// les familles de side-cars. Rend la liste des (slot_initial, marqueur).
fn populate(store: &SubstrateStore, n: u32, seed: u64) -> Vec<(u32, u16)> {
    let mut nodes: Vec<(u32, u16)> = Vec::new();
    let mut communities: Vec<(u32, u32)> = Vec::new();
    let mut centralities: Vec<(u32, u16)> = Vec::new();
    for i in 0..n {
        let node = store.create_node(&["Fiche"]);
        let slot = node.0 as u32;
        // Marqueur unique et stable : voyage dans le NodeRecord.
        let marker = (i as u16) + 1;
        // Communautés entrelacées pour forcer une vraie permutation.
        let cid = (i % 3) + 1 + ((seed as u32) % 2);
        communities.push((slot, cid));
        centralities.push((slot, marker));
        nodes.push((slot, marker));

        store.set_node_property(node, "embedding_vec", Value::Vector(vec_for(marker as u64).into()));
        store.set_node_property(node, "body", Value::String(body_for(marker as u64).into()));
    }
    store.writer().update_community_batch(communities).unwrap();
    store.writer().update_centrality_batch(centralities).unwrap();

    // Arêtes : le nœud de marqueur m a un out-degré déterministe m % 4,
    // réparti sur deux types d'arêtes (degrés totaux + typés).
    for &(slot, marker) in &nodes {
        for j in 0..(marker % 4) as u32 {
            let dst = nodes[((marker as u32 + j) % n) as usize].0;
            let ty = if j % 2 == 0 { "REL_A" } else { "REL_B" };
            store.create_edge(NodeId(slot as u64), NodeId(dst as u64), ty);
        }
    }

    // Engrams : l'engram 1 contient les marqueurs pairs, l'engram 2 les
    // multiples de 5. Bitsets posés en cohérence.
    let e1: Vec<NodeId> = nodes
        .iter()
        .filter(|(_, m)| m % 2 == 0)
        .map(|(s, _)| NodeId(*s as u64))
        .collect();
    let e2: Vec<NodeId> = nodes
        .iter()
        .filter(|(_, m)| m % 5 == 0)
        .map(|(s, _)| NodeId(*s as u64))
        .collect();
    store.set_engram_members(1, &e1).unwrap();
    store.set_engram_members(2, &e2).unwrap();
    for nid in e1.iter().chain(e2.iter()) {
        store.add_engram_bit(*nid, if e1.contains(nid) { 1 } else { 2 }).unwrap();
    }

    // Tier0 : table slot_to_offset dont les VALEURS sont les slots nœuds,
    // records dérivés du marqueur (permettent d'asserter « données
    // intactes, table remappée »).
    {
        let sub = store.writer().substrate();
        let guard = sub.lock();
        let mut zf = guard.open_zone(Zone::Tier0).unwrap();
        let s2o: Vec<u32> = nodes.iter().map(|(s, _)| *s).collect();
        let records: Vec<Tier0> = nodes
            .iter()
            .map(|(_, m)| Tier0([*m as u64, (*m as u64) << 32]))
            .collect();
        tier_persist::write_tier_zone::<Tier0>(
            &mut zf,
            TierMagic::Tier0,
            n,
            Some(&s2o),
            &records,
        )
        .unwrap();
    }

    nodes
}

/// Après permutation : map marqueur → slot courant, lue depuis la zone
/// nœuds elle-même (source de vérité).
fn markers_by_slot(store: &SubstrateStore) -> HashMap<u16, u32> {
    markers_by_slot_upto(store, store.slot_high_water())
}

fn markers_by_slot_upto(store: &SubstrateStore, hw: u32) -> HashMap<u16, u32> {
    let mut map = HashMap::new();
    for slot in 1..hw {
        let Some(rec) = store.writer().read_node(slot).unwrap() else {
            continue;
        };
        if rec.is_tombstoned() || rec.centrality_cached == 0 {
            continue;
        }
        assert!(
            map.insert(rec.centrality_cached, slot).is_none(),
            "marqueur {} dupliqué — permutation non bijective",
            rec.centrality_cached
        );
    }
    map
}

/// Retrouve LA colonne vec nœud de la base en analysant les noms de
/// fichiers (même chemin que le rejeu, qui n'a pas de registre). Les CRC
/// sont validés à l'ouverture : un `Some` prouve au passage l'invariant
/// I2 (en-tête cohérent avec le payload après remap).
fn open_node_vec_column(sub: &SubstrateFile) -> VecColumnReader {
    let mut found = None;
    for entry in std::fs::read_dir(sub.path()).unwrap().flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(spec) = VecColSpec::parse_filename(name)
            && spec.entity_kind == obrain_substrate::vec_column::EntityKind::Node
        {
            found = Some(spec);
        }
    }
    let spec = found.expect("aucune colonne vec nœud sur disque");
    VecColumnReader::open(sub, spec)
        .unwrap()
        .expect("colonne vec illisible (CRC invalide) après remap")
}

fn open_node_blob_column(sub: &SubstrateFile) -> BlobColumnReader {
    let mut found = None;
    for entry in std::fs::read_dir(sub.path()).unwrap().flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(spec) = BlobColSpec::parse_idx_filename(name)
            && spec.entity_kind == obrain_substrate::vec_column::EntityKind::Node
        {
            found = Some(spec);
        }
    }
    let spec = found.expect("aucune colonne blob nœud sur disque");
    BlobColumnReader::open(sub, spec)
        .unwrap()
        .expect("colonne blob illisible (CRC invalide) après remap")
}

/// Point d'entrée ENCADRÉ (flush avant, tri, flush après) — le seul
/// supporté. Appeler `writer().bulk_sort_by_hilbert` directement laisse
/// dans la fenêtre de rejeu des enregistrements pré-permutation que le
/// prochain redémarrage réécrirait par-dessus la nouvelle disposition ;
/// le writer refuse désormais ce cas.
fn bulk_sort(store: &SubstrateStore) {
    store.repermuter_par_hilbert(6, 32).unwrap();
}

// ---------------------------------------------------------------------------
// T3 — le test-preuve : veccol suit bulk_sort (ÉCHOUAIT sur HEAD)
// ---------------------------------------------------------------------------

#[test]
fn t3_veccol_suit_bulk_sort() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let nodes = populate(&store, 30, 0);

    bulk_sort(&store);

    let by_marker = markers_by_slot(&store);
    assert_eq!(by_marker.len(), nodes.len(), "des nœuds ont disparu au tri");

    let pk = obrain_common::types::PropertyKey::new("embedding_vec");
    let mut moved = 0u32;
    for &(old_slot, marker) in &nodes {
        let new_slot = by_marker[&marker];
        if new_slot != old_slot {
            moved += 1;
        }
        let got = store
            .get_node_property(NodeId(new_slot as u64), &pk)
            .unwrap_or_else(|| panic!("marqueur {marker}: embedding absent au slot {new_slot}"));
        let Value::Vector(v) = got else {
            panic!("marqueur {marker}: valeur non vectorielle {got:?}")
        };
        assert_eq!(
            v.as_ref(),
            &vec_for(marker as u64)[..],
            "marqueur {marker}: l'embedding au slot {new_slot} appartient à un AUTRE nœud"
        );
    }
    assert!(
        moved > 0,
        "le tri n'a déplacé aucun slot — test non probant, renforcer l'entrelacement"
    );
}

// ---------------------------------------------------------------------------
// T1 — property-test : chaque famille de side-car suit chaque nœud
// ---------------------------------------------------------------------------

#[test]
fn t1_propriete_la_donnee_suit_le_noeud() {
    for seed in [0u64, 1, 2] {
        let store = SubstrateStore::open_tempfile().unwrap();
        let n = 24 + (seed as u32) * 7;
        let nodes = populate(&store, n, seed);

        // États attendus, capturés AVANT le tri.
        let engram1_before: Vec<u16> = {
            let by_slot: HashMap<u32, u16> = nodes.iter().map(|&(s, m)| (s, m)).collect();
            store
                .engram_members(1)
                .unwrap()
                .unwrap()
                .iter()
                .map(|nid| by_slot[&(nid.0 as u32)])
                .collect()
        };

        bulk_sort(&store);
        let by_marker = markers_by_slot(&store);
        assert_eq!(by_marker.len(), nodes.len(), "seed {seed}: nœuds perdus");

        let pk_vec = obrain_common::types::PropertyKey::new("embedding_vec");
        let pk_body = obrain_common::types::PropertyKey::new("body");
        for &(_, marker) in &nodes {
            let slot = by_marker[&marker];
            let nid = NodeId(slot as u64);

            // veccol
            let Some(Value::Vector(v)) = store.get_node_property(nid, &pk_vec) else {
                panic!("seed {seed} marqueur {marker}: embedding absent")
            };
            assert_eq!(v.as_ref(), &vec_for(marker as u64)[..], "seed {seed}: veccol");

            // blobcol
            let Some(Value::String(s)) = store.get_node_property(nid, &pk_body) else {
                panic!("seed {seed} marqueur {marker}: body absent")
            };
            assert_eq!(s.as_ref(), body_for(marker as u64), "seed {seed}: blobcol");

            // bitset engram : membre ⇒ bit posé, non-membre ⇒ 0.
            let bits = store.engram_bitset(nid).unwrap();
            let expected_member = marker % 2 == 0 || marker % 5 == 0;
            assert_eq!(
                bits != 0,
                expected_member,
                "seed {seed} marqueur {marker}: bitset engram au mauvais slot"
            );
        }

        // membres engram : mêmes marqueurs qu'avant le tri.
        let engram1_after: Vec<u16> = store
            .engram_members(1)
            .unwrap()
            .unwrap()
            .iter()
            .map(|nid| {
                let rec = store.writer().read_node(nid.0 as u32).unwrap().unwrap();
                rec.centrality_cached
            })
            .collect();
        let (mut b, mut a) = (engram1_before.clone(), engram1_after.clone());
        b.sort_unstable();
        a.sort_unstable();
        assert_eq!(b, a, "seed {seed}: la liste de membres ne suit pas les nœuds");

        // degrés (totaux + typés) : lus directement dans les fichiers
        // (CRC déjà valide — le remap écrit l'en-tête en dernier).
        {
            let sub = store.writer().substrate();
            let guard = sub.lock();
            let col = DegreeColumn::open(&guard)
                .unwrap()
                .expect("colonne de degrés absente/CRC invalide après remap");
            for &(_, marker) in &nodes {
                let slot = by_marker[&marker];
                assert_eq!(
                    col.out_degree(slot),
                    (marker % 4) as u32,
                    "seed {seed} marqueur {marker}: out-degré au mauvais slot"
                );
            }
            let typed = DegreeColumn::open_by_filename(
                &guard,
                &obrain_substrate::typed_degree::filename_for_type("REL_A"),
            )
            .unwrap()
            .expect("colonne typée REL_A absente/CRC invalide après remap");
            for &(_, marker) in &nodes {
                let slot = by_marker[&marker];
                // j pair → REL_A : ceil((m % 4) / 2)
                let expected = ((marker % 4) as u32).div_ceil(2);
                assert_eq!(
                    typed.out_degree(slot),
                    expected,
                    "seed {seed} marqueur {marker}: degré typé au mauvais slot"
                );
            }

            // tier0 : table slot_to_offset remappée, records INTACTS.
            let zf = guard.open_zone(Zone::Tier0).unwrap();
            let (tn, s2o, records) =
                tier_persist::read_tier_zone::<Tier0>(&zf, TierMagic::Tier0)
                    .unwrap()
                    .expect("tier0 illisible après remap (CRC)");
            assert_eq!(tn, nodes.len() as u32);
            let s2o = s2o.unwrap();
            for (dense, &(_, marker)) in nodes.iter().enumerate() {
                assert_eq!(
                    s2o[dense], by_marker[&marker],
                    "seed {seed} marqueur {marker}: slot_to_offset[{dense}] non remappé"
                );
                assert_eq!(
                    records[dense].0[0], marker as u64,
                    "seed {seed}: les DONNÉES tier0 ont bougé (seule la table doit changer)"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// T4 — golden vector_top_k avant/après repermutation (dual-key prod)
// ---------------------------------------------------------------------------

#[test]
fn t4_golden_vector_top_k_avant_apres() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let nodes = populate(&store, 40, 1);
    let query = vec_for(9_999);
    let k = 10;

    let avant = store.vector_top_k("Fiche", "embedding_vec", "embedding", &query, k, |_| true);
    assert_eq!(avant.len(), k);

    bulk_sort(&store);
    let by_marker = markers_by_slot(&store);
    assert_eq!(by_marker.len(), nodes.len());

    let apres = store.vector_top_k("Fiche", "embedding_vec", "embedding", &query, k, |_| true);
    assert_eq!(apres.len(), k);

    // Parité des SCORES (les similarités sont l'invariant du recall).
    // Les offsets de tie-break changent avec la permutation — DOCUMENTÉ :
    // le tri secondaire `(sim desc, offset asc)` départage les ex æquo
    // par slot, et les slots viennent d'être réassignés.
    let sims_avant: Vec<f32> = avant.iter().map(|(_, s)| s.expect("score NULL inattendu")).collect();
    let sims_apres: Vec<f32> = apres.iter().map(|(_, s)| s.expect("score NULL inattendu")).collect();
    for (i, (a, b)) in sims_avant.iter().zip(sims_apres.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-6,
            "rang {i}: similarité divergente après repermutation ({a} vs {b})"
        );
    }

    // Auto-cohérence : chaque résultat post-tri porte bien l'embedding
    // dont le cosinus vaut le score annoncé.
    let pk_vec = obrain_common::types::PropertyKey::new("embedding_vec");
    for (nid, sim) in &apres {
        let Some(Value::Vector(v)) = store.get_node_property(*nid, &pk_vec) else {
            panic!("résultat {nid:?} sans embedding")
        };
        let recomputed = cosine(&query, &v);
        assert!(
            (recomputed - sim.unwrap()).abs() < 1e-6,
            "résultat {nid:?}: score {sim:?} ≠ cosinus recalculé {recomputed}"
        );
    }
}

// ---------------------------------------------------------------------------
// compact_community — relocations partielles, hw qui croît
// ---------------------------------------------------------------------------

#[test]
fn compact_community_relocations_partielles_remappe_les_sidecars() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let nodes = populate(&store, 20, 0);
    store.flush().unwrap();
    let hw_avant = store.slot_high_water();

    // Compacte la communauté 1 : relocations PARTIELLES (seuls ses membres
    // bougent, vers une plage fraîche en queue — hw CROÎT, ensure_room).
    let result = store
        .writer()
        .compact_community(1, hw_avant, store.edge_slot_high_water(), 4, 32)
        .unwrap();
    assert!(
        !result.relocations.is_empty(),
        "aucune relocation — test non probant"
    );
    assert!(result.new_node_hw > hw_avant, "hw doit croître (plage fraîche)");

    // Le store n'a pas d'API de publication de compaction (le warden
    // actuel ne passe que par bulk_sort), donc son allocateur ignore le
    // nouveau hw et `is_live_on_disk` refuserait tout id au-delà : les
    // assertions passent DIRECTEMENT par les colonnes — de toute façon le
    // niveau où se joue l'invariant.
    let by_marker = markers_by_slot_upto(&store, result.new_node_hw);
    assert_eq!(by_marker.len(), nodes.len(), "nœuds perdus à la compaction");
    assert!(
        by_marker.values().any(|&s| s >= hw_avant),
        "aucun nœud n'a atterri dans la plage fraîche — test non probant"
    );

    let sub_arc = store.writer().substrate();
    let sub = sub_arc.lock();
    let vec_col = open_node_vec_column(&sub);
    let blob_col = open_node_blob_column(&sub);
    for &(_, marker) in &nodes {
        let slot = by_marker[&marker];
        assert_eq!(
            vec_col.read_slot_f32(slot).unwrap_or_else(|| panic!(
                "marqueur {marker}: slot {slot} hors de la colonne après compaction"
            )),
            &vec_for(marker as u64)[..],
            "marqueur {marker}: veccol non remappée par la compaction"
        );
        let raw = blob_col
            .read_slot(slot)
            .unwrap_or_else(|| panic!("marqueur {marker}: blob absent au slot {slot}"));
        assert!(
            String::from_utf8_lossy(raw).contains(&format!("corps-{marker}-")),
            "marqueur {marker}: blobcol non remappée par la compaction"
        );
    }
}

// ---------------------------------------------------------------------------
// Cas limites : store vide, un seul nœud, tout-zéro reste absent,
// dimension figée, second tri no-op
// ---------------------------------------------------------------------------

#[test]
fn cas_limites_store_vide_et_singleton() {
    // hw = 0 : early-return, aucun side-car touché.
    let store = SubstrateStore::open_tempfile().unwrap();
    store.repermuter_par_hilbert(6, 32).unwrap();

    // Un seul nœud : permutation identité (sentinelle 0 → 0, nœud pinné).
    let n = store.create_node(&["Solo"]);
    store.set_node_property(n, "embedding_vec", Value::Vector(vec_for(42).into()));
    bulk_sort(&store);
    let pk = obrain_common::types::PropertyKey::new("embedding_vec");
    let Some(Value::Vector(v)) = store.get_node_property(n, &pk) else {
        panic!("singleton: embedding perdu")
    };
    assert_eq!(v.as_ref(), &vec_for(42)[..]);
}

/// Un slot SANS embedding ne doit jamais hériter de la donnée d'un autre
/// nœud — et les chemins qui appliquent la sentinelle « tout-zéro ==
/// absent » (`iter_props_for_entity`, `vector_top_k`) doivent continuer à
/// le voir absent.
///
/// NUANCE DE FORMAT (documentée, pré-existante) : la colonne vec est
/// DENSE et sans bitmap de présence. Un slot au-delà de `n_slots` est
/// « absent par la borne » ; un slot INTÉRIEUR tout-zéro est « absent par
/// convention » — convention honorée par `read_nonzero` /
/// `iter_props_for_entity` / `vector_top_k`, mais PAS par
/// `vec_columns.read`, sur lequel s'appuie `get_node_property` (déjà le
/// cas avant ce chantier pour les trous d'écriture épars, cf.
/// `sparse_writes_are_zero_filled_between`). Comme la donnée doit suivre
/// son nœud vers des slots plus hauts, le remap étend la couverture : un
/// slot jadis hors borne peut devenir intérieur, donc « présent à zéro »
/// via ce seul chemin. Aucune donnée n'est inventée — le vecteur y est
/// tout-zéro — mais l'assertion porte ici sur l'invariant qui compte.
#[test]
fn veccol_slot_sans_embedding_n_herite_jamais_de_la_donnee_d_autrui() {
    // La sentinelle « tout-zéro == absent » doit voyager sans jamais
    // devenir la donnée d'autrui, et la dimension de la colonne reste figée.
    let store = SubstrateStore::open_tempfile().unwrap();
    let nodes = populate(&store, 12, 0);
    // Nœud supplémentaire SANS embedding (slot grandi par les écritures
    // voisines → ligne tout-zéro dans la colonne).
    let sans = store.create_node(&["Fiche"]);
    let marker_sans = 999u16;
    store
        .writer()
        .update_centrality_batch(vec![(sans.0 as u32, marker_sans)])
        .unwrap();
    store
        .writer()
        .update_community_batch(vec![(sans.0 as u32, 2)])
        .unwrap();

    bulk_sort(&store);
    let by_marker = markers_by_slot(&store);
    let slot_sans = by_marker[&marker_sans];
    let pk = obrain_common::types::PropertyKey::new("embedding_vec");

    // (1) Invariant central : la ligne du nœud sans embedding est
    //     TOUT-ZÉRO — jamais le vecteur d'un autre nœud.
    {
        let sub_arc = store.writer().substrate();
        let sub = sub_arc.lock();
        let col = open_node_vec_column(&sub);
        if let Some(v) = col.read_slot_f32(slot_sans) {
            assert!(
                v.iter().all(|x| *x == 0.0),
                "le slot sans embedding a hérité de la donnée d'un autre nœud : {v:?}"
            );
        }
    }

    // (2) Les chemins à sentinelle continuent de le voir ABSENT.
    let query = vec_for(1);
    let top = store.vector_top_k("Fiche", "embedding_vec", "embedding", &query, 50, |_| true);
    assert!(
        !top.iter().any(|(nid, _)| nid.0 == slot_sans as u64),
        "un slot tout-zéro est remonté dans vector_top_k après remap"
    );

    // (3) Les autres embeddings sont toujours là, à la même dimension.
    for &(_, marker) in &nodes {
        let slot = by_marker[&marker];
        let Some(Value::Vector(v)) = store.get_node_property(NodeId(slot as u64), &pk) else {
            panic!("marqueur {marker}: embedding absent")
        };
        assert_eq!(v.len(), DIM, "dimension de colonne modifiée par le remap");
        assert_eq!(v.as_ref(), &vec_for(marker as u64)[..]);
    }
}

#[test]
fn second_tri_est_un_noop_et_les_sidecars_restent_corrects() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let nodes = populate(&store, 18, 0);
    bulk_sort(&store);
    let after_first = markers_by_slot(&store);

    // Second appel : HILBERT_SORTED est posé → no-op complet (idempotence
    // du chemin vif ; l'idempotence du rejeu est couverte par les tests
    // unitaires d'epoch dans slot_remap.rs).
    store.repermuter_par_hilbert(6, 32).unwrap();
    let after_second = markers_by_slot(&store);
    assert_eq!(after_first, after_second, "le second tri devait être un no-op");

    let pk = obrain_common::types::PropertyKey::new("embedding_vec");
    for &(_, marker) in &nodes {
        let slot = after_second[&marker];
        let Some(Value::Vector(v)) = store.get_node_property(NodeId(slot as u64), &pk) else {
            panic!("marqueur {marker}: embedding absent après double tri")
        };
        assert_eq!(v.as_ref(), &vec_for(marker as u64)[..]);
    }
}

// ---------------------------------------------------------------------------
// Adversarial : tombstones, colonnes multiples, garde EntityKind::Edge,
// sentinelle d'absence blob, ordre I3 observable
// ---------------------------------------------------------------------------

/// Les nœuds supprimés partent en QUEUE au tri Hilbert. Leur départ ne
/// doit jamais décaler la donnée des nœuds VIVANTS — le piège classique
/// d'un remap qui compacterait au lieu de permuter.
#[test]
fn tombstones_ne_decalent_pas_la_donnee_des_vivants() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let nodes = populate(&store, 24, 0);

    // Supprime un nœud sur quatre.
    let mut vivants: Vec<(u32, u16)> = Vec::new();
    for &(slot, marker) in &nodes {
        if marker % 4 == 0 {
            assert!(store.delete_node(NodeId(slot as u64)), "suppression refusée");
        } else {
            vivants.push((slot, marker));
        }
    }
    assert!(!vivants.is_empty() && vivants.len() < nodes.len());

    bulk_sort(&store);
    let by_marker = markers_by_slot(&store);

    let pk_vec = obrain_common::types::PropertyKey::new("embedding_vec");
    for &(_, marker) in &vivants {
        let slot = by_marker
            .get(&marker)
            .unwrap_or_else(|| panic!("marqueur {marker} vivant a disparu du tri"));
        let Some(Value::Vector(v)) = store.get_node_property(NodeId(*slot as u64), &pk_vec) else {
            panic!("marqueur {marker}: embedding absent après tri avec tombstones")
        };
        assert_eq!(
            v.as_ref(),
            &vec_for(marker as u64)[..],
            "marqueur {marker}: un tombstone a décalé la donnée d'un vivant"
        );
    }
}

/// Plusieurs colonnes vec nœud doivent TOUTES suivre (le registre itère
/// bien l'ensemble), et une colonne d'ARÊTES ne doit PAS être touchée :
/// la permutation porte sur les slots nœuds, pas sur les arêtes.
#[test]
fn colonnes_multiples_suivent_et_colonne_arete_reste_intacte() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let nodes = populate(&store, 20, 0);

    // Deuxième colonne vec nœud, dimension différente.
    for &(slot, marker) in &nodes {
        let v: Vec<f32> = vec![marker as f32; 8];
        store.set_node_property(NodeId(slot as u64), "vec_aux", Value::Vector(v.into()));
    }
    // Colonne vec d'ARÊTES : capturée avant, doit être identique après.
    let e = store.create_edge(
        NodeId(nodes[0].0 as u64),
        NodeId(nodes[1].0 as u64),
        "REL_A",
    );
    let vec_arete: Vec<f32> = vec![42.0; 4];
    store.set_edge_property(e, "edge_vec", Value::Vector(vec_arete.clone().into()));

    bulk_sort(&store);
    let by_marker = markers_by_slot(&store);

    let pk_aux = obrain_common::types::PropertyKey::new("vec_aux");
    let pk_vec = obrain_common::types::PropertyKey::new("embedding_vec");
    for &(_, marker) in &nodes {
        let slot = by_marker[&marker];
        let nid = NodeId(slot as u64);
        let Some(Value::Vector(aux)) = store.get_node_property(nid, &pk_aux) else {
            panic!("marqueur {marker}: vec_aux absent")
        };
        assert_eq!(aux.as_ref(), &vec![marker as f32; 8][..], "vec_aux non remappée");
        let Some(Value::Vector(v)) = store.get_node_property(nid, &pk_vec) else {
            panic!("marqueur {marker}: embedding_vec absent")
        };
        assert_eq!(v.as_ref(), &vec_for(marker as u64)[..], "embedding_vec non remappée");
    }

    // La colonne d'arêtes est inchangée : même slot d'arête, même valeur.
    let pk_edge = obrain_common::types::PropertyKey::new("edge_vec");
    let Some(Value::Vector(got)) = store.get_edge_property(e, &pk_edge) else {
        panic!("edge_vec absente après le tri — la colonne d'arêtes a été touchée")
    };
    assert_eq!(
        got.as_ref(),
        &vec_arete[..],
        "la colonne d'ARÊTES a été permutée alors que seuls les slots nœuds bougent"
    );
}

/// Sentinelle d'absence des colonnes blob (`len == 0`) : elle doit
/// voyager comme une donnée ordinaire, sans jamais devenir présente ni
/// masquer le blob d'un autre nœud.
#[test]
fn sentinelle_d_absence_blob_voyage_avec_le_noeud() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let nodes = populate(&store, 16, 0);

    // Efface le blob d'un nœud sur trois (écriture d'une valeur courte :
    // la valeur blob doit disparaître, pas rester masquante).
    let mut effaces: Vec<u16> = Vec::new();
    for &(slot, marker) in &nodes {
        if marker % 3 == 0 {
            store.set_node_property(NodeId(slot as u64), "body", Value::String("court".into()));
            effaces.push(marker);
        }
    }
    assert!(!effaces.is_empty());

    bulk_sort(&store);
    let by_marker = markers_by_slot(&store);

    let pk_body = obrain_common::types::PropertyKey::new("body");
    for &(_, marker) in &nodes {
        let slot = by_marker[&marker];
        let Some(Value::String(s)) = store.get_node_property(NodeId(slot as u64), &pk_body) else {
            panic!("marqueur {marker}: body absent après remap")
        };
        if effaces.contains(&marker) {
            assert_eq!(
                s.as_ref() as &str,
                "court",
                "marqueur {marker}: l'ancien blob masque la valeur courte après remap"
            );
        } else {
            assert_eq!(
                s.as_ref(),
                body_for(marker as u64),
                "marqueur {marker}: blob d'un autre nœud"
            );
        }
    }
}

/// Ordre I3 observable : après une passe réussie, l'epoch de remap est
/// durable ET le flag HILBERT_SORTED est posé. L'ordre inverse (flag
/// avant msync) était la fenêtre W2/W3 corrigée par ce chantier.
#[test]
fn apres_la_passe_epoch_durable_et_flag_sont_coherents() {
    use obrain_substrate::meta::meta_flags;
    let store = SubstrateStore::open_tempfile().unwrap();
    populate(&store, 12, 0);

    let sub_arc = store.writer().substrate();
    {
        let sub = sub_arc.lock();
        assert_eq!(
            obrain_substrate::slot_remap::current_remap_epoch(&sub).unwrap(),
            0,
            "base neuve : aucun epoch de remap"
        );
    }

    bulk_sort(&store);

    let sub = sub_arc.lock();
    assert_eq!(
        obrain_substrate::slot_remap::current_remap_epoch(&sub).unwrap(),
        1,
        "l'epoch durable doit être posé à la fin de la passe"
    );
    assert!(
        sub.meta_header().flags & meta_flags::HILBERT_SORTED != 0,
        "HILBERT_SORTED doit être posé après la passe"
    );
}

// ---------------------------------------------------------------------------
// Conditions de re-revue : les fenêtres de crash que la revue adversariale
// a désignées comme non exercées
// ---------------------------------------------------------------------------

/// Rejeu depuis un offset de checkpoint NON NUL — le mode de PRODUCTION.
///
/// C'est le scénario du bloquant R9 : trafic ordinaire, tri réussi, crash
/// PLUS TARD, puis rejeu. Avant le correctif, le rejeu repartait d'avant
/// le tri et réécrivait des identifiants pré-permutation dans une zone
/// post-permutation. Désormais l'encadrement par flushes fait que la
/// fenêtre de rejeu ne contient jamais d'enregistrement pré-tri.
#[test]
fn rejeu_depuis_offset_de_checkpoint_apres_un_tri_termine() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let attendu: Vec<(u16, Vec<f32>)>;
    let offset_rejeu: u64;
    {
        let store = SubstrateStore::create(&path).unwrap();
        let nodes = populate(&store, 24, 0);

        // Tri via le point d'entrée encadré.
        store.repermuter_par_hilbert(6, 32).unwrap();

        // TRAFIC APRÈS LE TRI — en numérotation POST-permutation.
        let by_marker = markers_by_slot(&store);
        for &(_, marker) in nodes.iter().take(5) {
            let slot = by_marker[&marker];
            store.set_node_property(
                NodeId(slot as u64),
                "apres_tri",
                Value::Int64(marker as i64),
            );
        }

        // L'offset d'où le rejeu repartira : celui du dernier flush.
        offset_rejeu = store.offset_wal_du_dernier_flush();
        assert!(offset_rejeu > 0, "un checkpoint de prod n'est jamais a 0");

        attendu = nodes
            .iter()
            .map(|&(_, m)| (m, vec_for(m as u64)))
            .collect();

        // « Crash » : on lâche le store sans flush final.
        drop(store);
    }

    // Rejeu depuis l'offset de checkpoint, deux fois de suite.
    for passe in 1..=2 {
        let sub = SubstrateFile::open(&path).unwrap();
        obrain_substrate::replay_from(&sub, offset_rejeu).unwrap();
        drop(sub);

        let store = SubstrateStore::open(&path).unwrap();
        let by_marker = markers_by_slot(&store);
        let col = {
            let sub_arc = store.writer().substrate();
            let sub = sub_arc.lock();
            open_node_vec_column(&sub)
        };
        for (marker, embedding) in &attendu {
            let slot = by_marker
                .get(marker)
                .unwrap_or_else(|| panic!("passe {passe}: marqueur {marker} disparu"));
            assert_eq!(
                col.read_slot_f32(*slot).unwrap_or_else(|| panic!(
                    "passe {passe}: marqueur {marker} hors colonne"
                )),
                &embedding[..],
                "passe {passe}: l'embedding du marqueur {marker} ne suit plus son noeud \
                 apres un rejeu depuis un offset de checkpoint"
            );
        }
        drop(store);
    }
}

/// Rejeu MULTI-GÉNÉRATIONS : deux tris successifs, puis rejeu depuis
/// avant le premier. Chaque side-car ne doit avancer que d'une
/// génération à la fois, gardé par son propre epoch d'en-tête.
#[test]
fn rejeu_multi_generations_deux_tris_successifs() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let attendu: Vec<(u16, Vec<f32>)>;
    let offset_avant_premier_tri: u64;
    {
        let store = SubstrateStore::create(&path).unwrap();
        let nodes = populate(&store, 20, 1);
        store.flush().unwrap();
        offset_avant_premier_tri = store.offset_wal_du_dernier_flush();

        // Génération 1.
        store.repermuter_par_hilbert(6, 32).unwrap();
        // Génération 2 : il faut d'abord invalider le drapeau de tri.
        store.invalidate_layout_flag();
        store.repermuter_par_hilbert(5, 16).unwrap();

        let epoch = {
            let sub_arc = store.writer().substrate();
            let sub = sub_arc.lock();
            obrain_substrate::slot_remap::current_remap_epoch(&sub).unwrap()
        };
        assert_eq!(epoch, 2, "deux tris => deux generations d'epoch");

        attendu = nodes
            .iter()
            .map(|&(_, m)| (m, vec_for(m as u64)))
            .collect();
        drop(store);
    }

    // Rejeu depuis AVANT la première génération.
    let sub = SubstrateFile::open(&path).unwrap();
    obrain_substrate::replay_from(&sub, offset_avant_premier_tri).unwrap();
    drop(sub);

    let store = SubstrateStore::open(&path).unwrap();
    let by_marker = markers_by_slot(&store);
    let sub_arc = store.writer().substrate();
    let sub = sub_arc.lock();
    let col = open_node_vec_column(&sub);
    for (marker, embedding) in &attendu {
        let slot = by_marker[marker];
        assert_eq!(
            col.read_slot_f32(slot)
                .unwrap_or_else(|| panic!("marqueur {marker} hors colonne")),
            &embedding[..],
            "marqueur {marker}: le rejeu multi-generations a desynchronise la colonne"
        );
    }
}

/// Condition 4 — un index tiered issu de `new()` (donc non lié à un
/// epoch) ne doit pas pouvoir persister sur une base déjà repermutée :
/// c'est exactement le chemin du rebuild quand tier0 est absent.
#[test]
fn un_index_tiered_non_lie_refuse_de_persister_apres_un_remap() {
    use obrain_core::index::vector::VectorIndex;
    use obrain_substrate::retrieval::SubstrateTieredIndex;
    use obrain_substrate::tiers::L2_DIM;

    let td = tempfile::tempdir().unwrap();
    let sub = SubstrateFile::create(td.path()).unwrap();

    // Base neuve (epoch 0) : un index non lié persiste normalement.
    let index = SubstrateTieredIndex::new(L2_DIM);
    index.insert(1, &vec![0.5f32; L2_DIM]);
    index
        .persist_to_zones(&sub)
        .expect("sur une base jamais repermutee, rien ne change");

    // La base est repermutée.
    obrain_substrate::slot_remap::commit_remap_epoch(&sub, 1).unwrap();

    // Un index NON LIÉ refuse désormais de persister.
    let orphelin = SubstrateTieredIndex::new(L2_DIM);
    orphelin.insert(1, &vec![0.25f32; L2_DIM]);
    let err = orphelin
        .persist_to_zones(&sub)
        .expect_err("un index non lie ne doit pas ecraser une table remappee");
    assert!(
        format!("{err:?}").contains("non lie"),
        "le refus doit nommer la cause : {err:?}"
    );

    // Lié explicitement à la base, il persiste.
    let lie = SubstrateTieredIndex::new_lie_a_la_base(&sub, L2_DIM).unwrap();
    lie.insert(1, &vec![0.75f32; L2_DIM]);
    lie.persist_to_zones(&sub)
        .expect("un index lie atteste avoir ete bati apres la repermutation");
}
