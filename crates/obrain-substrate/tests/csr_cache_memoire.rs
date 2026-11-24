//! Le cache CSR doit rester exact pour un store SANS fichier WAL
//! (`SubstrateStore::open_memory`, chemin wasm32) — #292, volet justesse.
//!
//! `csr_cached` se clé sur `(feature_epoch, taille du WAL)`. Sans fichier WAL
//! la taille vaut 0 ; la clé retombe sur l'époque seule, que ni
//! `tombstone_edge`, ni `tombstone_node`, ni `update_edge`, ni les écritures de
//! poids n'incrémentent. Chaque test compare le CSR servi par le cache à un
//! `CsrAdjacency::build` neuf, sur les deux types de store (mémoire et fichier).
use obrain_common::types::{EdgeId, NodeId};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::geometric::CsrAdjacency;
use obrain_substrate::store::SubstrateStore;
use std::sync::Arc;

/// `(store, noeuds, aretes)` ; garde le répertoire temporaire en vie.
type Banc = (
    SubstrateStore,
    [NodeId; 3],
    [EdgeId; 2],
    Option<tempfile::TempDir>,
);

fn banc(en_memoire: bool) -> Banc {
    let (s, td) = if en_memoire {
        (SubstrateStore::open_memory().unwrap(), None)
    } else {
        let td = tempfile::tempdir().unwrap();
        (
            SubstrateStore::create(td.path().join("kb")).unwrap(),
            Some(td),
        )
    };
    let n = [
        s.create_node(&["N"]),
        s.create_node(&["N"]),
        s.create_node(&["N"]),
    ];
    let e = [
        s.create_edge(n[0], n[1], "LIEN"),
        s.create_edge(n[1], n[2], "LIEN"),
    ];
    (s, n, e, td)
}

fn egal(a: &CsrAdjacency, b: &CsrAdjacency) -> bool {
    a.row_offsets == b.row_offsets && a.edges == b.edges && a.slot_high_water == b.slot_high_water
}

/// Remplit le cache, applique `mutation`, puis exige que le cache serve
/// exactement ce qu'un build neuf produit. Rend `(avant, apres)`.
fn verifie(
    en_memoire: bool,
    mutation: impl FnOnce(&SubstrateStore, [NodeId; 3], [EdgeId; 2]),
) -> (Arc<CsrAdjacency>, Arc<CsrAdjacency>) {
    let (s, n, e, _td) = banc(en_memoire);
    let avant = s.csr_cached();
    assert_eq!(avant.edges.len(), 2, "préalable : 2 arêtes");
    assert!(egal(&avant, &CsrAdjacency::build(&s)));
    mutation(&s, n, e);
    let apres = s.csr_cached();
    let neuf = CsrAdjacency::build(&s);
    assert!(
        egal(&apres, &neuf),
        "CSR périmé (memoire={en_memoire}) : cache {} arêtes, build neuf {}",
        apres.edges.len(),
        neuf.edges.len()
    );
    (avant, apres)
}

#[test]
fn suppression_d_arete() {
    for mem in [true, false] {
        let (_, apres) = verifie(mem, |s, _, e| assert!(s.delete_edge(e[0])));
        assert_eq!(apres.edges.len(), 1);
    }
}

#[test]
fn suppression_d_arete_puis_reactivation() {
    for mem in [true, false] {
        // `revive_edge` ne rechaîne pas (voir sa doc) : le CSR reste à 1 arête,
        // mais il doit toujours égaler un build neuf.
        let (_, apres) = verifie(mem, |s, _, e| {
            assert!(s.delete_edge(e[0]));
            let _ = s.csr_cached();
            s.writer().revive_edge(e[0].0).unwrap();
        });
        assert_eq!(apres.edges.len(), 1);
    }
}

#[test]
fn mise_a_jour_d_arete_poids() {
    for mem in [true, false] {
        let (avant, apres) = verifie(mem, |s, _, e| {
            let mut rec = s.writer().read_edge(e[0].0).unwrap().unwrap();
            rec.weight_u16 = 12345;
            s.writer().update_edge(e[0].0, rec).unwrap();
        });
        assert_ne!(
            avant.edges, apres.edges,
            "le poids doit se voir dans le CSR"
        );
    }
}

#[test]
fn ecriture_de_poids_reinforce_synapse() {
    for mem in [true, false] {
        let (avant, apres) = verifie(mem, |s, _, e| {
            s.writer().reinforce_synapse(e[1].0, 4242).unwrap();
        });
        assert_ne!(avant.edges, apres.edges);
    }
}

#[test]
fn ecriture_de_poids_coact_reinforce_at() {
    for mem in [true, false] {
        let (avant, apres) = verifie(mem, |s, _, e| {
            s.writer().coact_reinforce_at(e[0].0, 777).unwrap();
        });
        assert_ne!(avant.edges, apres.edges);
    }
}

#[test]
fn ecriture_de_poids_decay_all_coact() {
    for mem in [true, false] {
        // Les arêtes de `banc` ne sont pas COACT : la décroissance est un no-op
        // de données, mais le CSR doit rester égal à un build neuf.
        let (_, apres) = verifie(mem, |s, _, _| {
            let _ = s.decay_all_coact(0.5);
        });
        assert_eq!(apres.edges.len(), 2);
    }
}

#[test]
fn suppression_de_noeud() {
    for mem in [true, false] {
        let (_, apres) = verifie(mem, |s, n, _| {
            assert!(s.delete_node(n[0]));
        });
        // Le CSR d'un noeud supprimé ne doit plus exposer ses arêtes sortantes.
        assert!(apres.edges.len() <= 2);
    }
}

#[test]
fn graphe_inchange_reutilise_le_meme_csr() {
    for mem in [true, false] {
        let (s, _, _, _td) = banc(mem);
        let a = s.csr_cached();
        let b = s.csr_cached();
        assert!(
            Arc::ptr_eq(&a, &b),
            "graphe inchangé : le cache doit servir"
        );
    }
}

/// Une écriture par famille : la révision structurelle doit avancer à chacune.
/// Un oubli sur un point d'écriture serait sinon invisible (CSR périmé servi).
#[test]
fn chaque_famille_d_ecriture_fait_avancer_la_revision() {
    for mem in [true, false] {
        let (s, n, e, _td) = banc(mem);
        let w = s.writer();
        let mut r = w.structure_rev();
        let mut avance = |nom: &str| {
            let r2 = w.structure_rev();
            assert!(
                r2 > r,
                "{nom} n'a pas fait avancer la révision (memoire={mem})"
            );
            r = r2;
        };
        let rec = w.read_edge(e[0].0).unwrap().unwrap();
        w.update_edge(e[0].0, rec).unwrap();
        avance("update_edge");
        w.reinforce_synapse(e[0].0, 10).unwrap();
        avance("reinforce_synapse");
        w.coact_reinforce_at(e[0].0, 10).unwrap();
        avance("coact_reinforce_at");
        w.decay_all_coact(30000, s.edge_slot_high_water(), 0)
            .unwrap();
        avance("decay_all_coact");
        w.tombstone_edge(e[1].0).unwrap();
        avance("tombstone_edge");
        w.revive_edge(e[1].0).unwrap();
        avance("revive_edge");
        w.tombstone_node(n[2].0 as u32).unwrap();
        avance("tombstone_node");
        w.revive_node(n[2].0 as u32).unwrap();
        avance("revive_node");
    }
}
