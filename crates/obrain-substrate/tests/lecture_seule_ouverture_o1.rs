//! ── L'OUVERTURE EN LECTURE SEULE NE BALAIE PLUS LA BASE, ET VOIT TOUJOURS
//!    UNE BASE INCOHÉRENTE ──────────────────────────────────────────────────
//!
//! Trois vérifications faisaient de l'ouverture un coût en O(N) (~8 ms par
//! million de nœuds) : le recomptage des compteurs d'instantané contre les
//! zones (décision du 2026-09-15), le relevé des filigranes (balayage arrière
//! depuis la capacité préallouée) et le CRC de la colonne de degrés. En lecture
//! seule, elles sont désormais différées ou bornées. Chaque test ci-dessous
//! CORROMPT l'élément qu'une vérification protège et montre que l'incohérence
//! est toujours vue — au premier lecteur au lieu de l'ouverture.

use obrain_common::types::NodeId;
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::{DictSnapshot, SubstrateStore};
use std::path::Path;

const N: usize = 300;

/// Base propre : `N` personnes, une arête `CONNAIT` vers la précédente, un
/// label `Ancien` sur un nœud sur trois.
fn base(chemin: &Path) {
    let s = SubstrateStore::create(chemin).expect("création");
    let mut prec: Option<NodeId> = None;
    for i in 0..N {
        let id = if i % 3 == 0 {
            s.create_node(&["Personne", "Ancien"])
        } else {
            s.create_node(&["Personne"])
        };
        if let Some(p) = prec {
            s.create_edge(id, p, "CONNAIT");
        }
        prec = Some(id);
    }
    s.flush().expect("flush");
    s.close().expect("close");
}

fn reecrire_dict(chemin: &Path, f: impl FnOnce(&mut DictSnapshot)) {
    let p = chemin.join("substrate.dict");
    let mut d = DictSnapshot::load(&p).expect("dict");
    f(&mut d);
    d.persist(&p).expect("réécrire le dict");
}

#[test]
fn ouvrir_une_base_propre_ne_lance_aucune_verification_pleine() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("propre");
    base(&chemin);

    let s = SubstrateStore::open_read_only(&chemin).unwrap();
    assert_eq!(
        s.nb_verifications_pleines(),
        0,
        "l'ouverture en lecture seule a balayé les zones pour vérifier les compteurs"
    );
    assert_eq!(s.node_count(), N);
    assert_eq!(
        s.nb_verifications_pleines(),
        1,
        "la vérification est due au premier lecteur des compteurs, une fois"
    );
    assert_eq!(s.edge_count(), N - 1);
    assert_eq!(
        s.nb_verifications_pleines(),
        1,
        "une seule fois par session"
    );
}

/// Les compteurs persistés MENTENT (la dérive « IN_MISSION 4660 pour 2347 »
/// qui a motivé la décision du 2026-09-15) : aucun compte faux n'est servi.
#[test]
fn des_compteurs_persistes_faux_ne_sont_jamais_servis() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("compteurs-faux");
    base(&chemin);
    reecrire_dict(&chemin, |d| {
        let c = d.counters.as_mut().expect("compteurs persistés");
        c.total_live_nodes = 4660;
        c.total_live_edges = 4660;
        for v in c.label_counts.iter_mut() {
            if *v > 0 {
                *v *= 2;
            }
        }
        for (_, v) in c.edge_type_counts.iter_mut() {
            *v += 1000;
        }
    });

    let s = SubstrateStore::open_read_only(&chemin).unwrap();
    assert_eq!(
        s.nb_verifications_pleines(),
        0,
        "rien n'est balayé à l'ouverture"
    );
    assert_eq!(s.node_count(), N, "total des nœuds");
    assert_eq!(s.edge_count(), N - 1, "total des arêtes");
    assert_eq!(s.node_count_by_label("Personne"), N, "compte par label");
    assert_eq!(
        s.node_count_by_label("Ancien"),
        N.div_ceil(3),
        "compte par label"
    );
    assert_eq!(
        s.live_count_by_edge_type("CONNAIT"),
        (N - 1) as u64,
        "compte par type d'arête"
    );
}

/// Chaque lecteur, pris SEUL en premier, déclenche la vérification : aucun
/// n'est une porte dérobée vers les compteurs non vérifiés.
#[test]
fn chaque_lecteur_de_compteurs_declenche_la_verification() {
    type Lecteur = fn(&SubstrateStore) -> u64;
    let lecteurs: [(&str, Lecteur); 4] = [
        ("node_count", |s| s.node_count() as u64),
        ("edge_count", |s| s.edge_count() as u64),
        ("node_count_by_label", |s| {
            s.node_count_by_label("Personne") as u64
        }),
        ("live_count_by_edge_type", |s| {
            s.live_count_by_edge_type("CONNAIT")
        }),
    ];
    let attendus = [N as u64, (N - 1) as u64, N as u64, (N - 1) as u64];
    for ((nom, lire), attendu) in lecteurs.into_iter().zip(attendus) {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("lecteur");
        base(&chemin);
        reecrire_dict(&chemin, |d| {
            let c = d.counters.as_mut().unwrap();
            c.total_live_nodes = 1;
            c.total_live_edges = 1;
            c.label_counts = [1; 64];
            for (_, v) in c.edge_type_counts.iter_mut() {
                *v = 1;
            }
        });
        let s = SubstrateStore::open_read_only(&chemin).unwrap();
        assert_eq!(lire(&s), attendu, "{nom} servi sans vérification");
    }
}

/// La borne d'allocation ET le dictionnaire sont en retard sur les zones (une
/// borne restaurée d'ailleurs, un fichier tronqué) : la sonde voit des
/// enregistrements vivants au-delà, et le relevé complet reprend.
#[test]
fn une_borne_d_allocation_en_retard_est_vue_et_les_noeuds_restent_visibles() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("borne-en-retard");
    base(&chemin);
    reecrire_dict(&chemin, |d| {
        d.next_node_id = 10;
        d.next_edge_id = 10;
    });
    // Même disposition que `BorneAllocation` : magie, nœud, arête (u64 natifs).
    let alloc = chemin.join("substrate.alloc");
    let mut octets = std::fs::read(&alloc).expect("substrate.alloc présent");
    octets[8..16].copy_from_slice(&10u64.to_ne_bytes());
    octets[16..24].copy_from_slice(&10u64.to_ne_bytes());
    std::fs::write(&alloc, &octets).unwrap();

    let s = SubstrateStore::open_read_only(&chemin).unwrap();
    assert_eq!(
        s.node_ids().len(),
        N,
        "des nœuds vivants au-delà de la borne sont devenus invisibles"
    );
    assert_eq!(s.node_count(), N);
    assert_eq!(s.edge_count(), N - 1);
}

/// La colonne de degrés a un CRC faux : elle n'est plus contrôlée à
/// l'ouverture, mais au premier degré lu — et les degrés restent exacts.
#[test]
fn une_colonne_de_degres_corrompue_est_vue_au_premier_degre_lu() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("degres");
    base(&chemin);
    let colonne = chemin.join("substrate.degrees.node.u32");
    let mut octets = std::fs::read(&colonne).unwrap();
    // Contenu ET CRC faux : un payload remis à zéro sous son en-tête d'origine.
    for b in octets.iter_mut().skip(64) {
        *b = 0;
    }
    std::fs::write(&colonne, &octets).unwrap();

    let s = SubstrateStore::open_read_only(&chemin).unwrap();
    let ids = s.node_ids();
    let sortants: usize = ids.iter().map(|&id| s.out_degree(id) as usize).sum();
    let entrants: usize = ids.iter().map(|&id| s.in_degree(id) as usize).sum();
    assert_eq!(
        sortants,
        N - 1,
        "degrés sortants exacts malgré la colonne corrompue"
    );
    assert_eq!(
        entrants,
        N - 1,
        "degrés entrants exacts malgré la colonne corrompue"
    );
}
