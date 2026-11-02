//! Les slots d'arête RÉSERVÉS mais jamais écrits ne sont pas des arêtes.
//!
//! Mesuré le 02/11/2026 sur la plateforme (`obrain compare`, 4 tests rouges) :
//! après réouverture, `next_edge_id` est rétabli à la borne du bloc réservé
//! (`TAILLE_BLOC_ALLOCATION` = 256) et la zone contient des records à zéro
//! (`src == 0 && dst == 0`, non tombstonés). `for_each_live_edge` les rendait
//! comme vivants : 1 arête réelle → 256 rendues ; en mode structurel les
//! fantômes partagent une clé et le `dedup` n'en laissait qu'un (+1 par store).
//!
//! L'invariant : ce que l'itérateur vivant rend est exactement ce que
//! `edge_count()` compte (compteur atomique entretenu à l'écriture).
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

fn nombre_vivantes(s: &SubstrateStore) -> usize {
    let mut n = 0usize;
    s.for_each_live_edge(|_slot, _rec| n += 1)
        .expect("for_each_live_edge");
    n
}

#[test]
fn l_iterateur_vivant_rend_exactement_edge_count_meme_apres_reouverture() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("t.obrain");
    {
        let s = SubstrateStore::create(&chemin).expect("create");
        let a = s.create_node(&["Function"]);
        let b = s.create_node(&["Function"]);
        s.create_edge(a, b, "CALLS");
        s.flush().expect("flush");
        assert_eq!(GraphStore::edge_count(&s), 1);
        assert_eq!(
            nombre_vivantes(&s),
            1,
            "avant réouverture : une seule arête vivante (borne des slots : {})",
            s.edge_slot_high_water()
        );
    }
    let r = SubstrateStore::open_read_only(&chemin).expect("open_read_only");
    assert_eq!(
        GraphStore::edge_count(&r),
        1,
        "le compteur survit à la réouverture"
    );
    assert_eq!(
        nombre_vivantes(&r),
        1,
        "après réouverture : les slots réservés par bloc mais jamais écrits ne sont pas des \
         arêtes (borne des slots : {})",
        r.edge_slot_high_water()
    );
    // Et aucun record rendu n'est le record à zéro d'un slot jamais écrit.
    r.for_each_live_edge(|slot, rec| {
        assert!(
            rec.src != 0 && rec.dst != 0,
            "slot {slot} rendu vivant avec src={} dst={} : un slot jamais écrit",
            rec.src,
            rec.dst
        );
    })
    .unwrap();
}

#[test]
fn une_arete_tombstonee_n_est_pas_rendue_mais_les_autres_le_restent() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("t.obrain");
    let s = SubstrateStore::create(&chemin).expect("create");
    let a = s.create_node(&["F"]);
    let b = s.create_node(&["F"]);
    let c = s.create_node(&["F"]);
    let e1 = s.create_edge(a, b, "CALLS");
    let _e2 = s.create_edge(b, c, "CALLS");
    s.flush().expect("flush");
    assert_eq!(nombre_vivantes(&s), 2);
    s.delete_edge(e1);
    s.flush().expect("flush");
    assert_eq!(GraphStore::edge_count(&s), 1);
    assert_eq!(
        nombre_vivantes(&s),
        1,
        "la tombstone est sautée, l'autre arête reste"
    );
}
