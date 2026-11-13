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

/// Base de `n` nœuds en chaîne (`CONNAIT` vers le PRÉCÉDENT), pour les tests de
/// troncature : assez grande pour que la troncature dépasse la tolérance.
fn base_de(chemin: &Path, n: usize) {
    let s = SubstrateStore::create(chemin).expect("création");
    let mut prec: Option<NodeId> = None;
    for _ in 0..n {
        let id = s.create_node(&["Personne"]);
        if let Some(p) = prec {
            s.create_edge(id, p, "CONNAIT");
        }
        prec = Some(id);
    }
    s.flush().expect("flush");
    s.close().expect("close");
}

fn lister(chemin: &Path) -> Vec<(String, u64)> {
    let mut v: Vec<_> = std::fs::read_dir(chemin)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                e.metadata().unwrap().len(),
            )
        })
        .collect();
    v.sort();
    v
}

/// DÉFAUT RELEVÉ À LA RELECTURE DE #118 : `substrate.nodes` réduit à 60 000
/// octets, la lecture seule rendait 1 499 nœuds sur 2 000 — sans erreur ni
/// avertissement. Elle refuse désormais, par une erreur nommée, sans rien écrire.
#[test]
fn une_zone_de_noeuds_tronquee_fait_echouer_l_ouverture_par_une_erreur_nommee() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("tronquee-noeuds");
    base_de(&chemin, 2000);
    std::fs::OpenOptions::new()
        .write(true)
        .open(chemin.join("substrate.nodes"))
        .unwrap()
        .set_len(60_000)
        .unwrap();
    let avant = lister(&chemin);

    let erreur = match SubstrateStore::open_read_only(&chemin) {
        Ok(s) => panic!(
            "ouverture acceptée : {} nœuds rendus sur 2000, sans erreur",
            s.node_count()
        ),
        Err(e) => e.to_string(),
    };
    assert!(
        erreur.contains("substrate.nodes") && erreur.contains("tronqué"),
        "refus non nommé : {erreur}"
    );
    assert_eq!(lister(&chemin), avant, "le refus a modifié le dossier");
}

#[test]
fn une_zone_d_aretes_tronquee_fait_echouer_l_ouverture_par_une_erreur_nommee() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("tronquee-aretes");
    base_de(&chemin, 2000);
    std::fs::OpenOptions::new()
        .write(true)
        .open(chemin.join("substrate.edges"))
        .unwrap()
        .set_len(3_600)
        .unwrap();
    let avant = lister(&chemin);

    let erreur = match SubstrateStore::open_read_only(&chemin) {
        Ok(_) => panic!("ouverture acceptée sur une zone d'arêtes tronquée"),
        Err(e) => e.to_string(),
    };
    assert!(
        erreur.contains("substrate.edges") && erreur.contains("tronqué"),
        "refus non nommé : {erreur}"
    );
    assert_eq!(lister(&chemin), avant, "le refus a modifié le dossier");
}

/// Une zone absente alors que le dictionnaire annonce des nœuds : même refus.
#[test]
fn une_zone_de_noeuds_absente_fait_echouer_l_ouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("sans-noeuds");
    base_de(&chemin, 2000);
    std::fs::remove_file(chemin.join("substrate.nodes")).unwrap();
    assert!(
        SubstrateStore::open_read_only(&chemin).is_err(),
        "ouverture acceptée sans zone de nœuds"
    );
}

/// Une base saine, de la taille des tests de troncature, s'ouvre : la garde ne
/// refuse pas une base légitime.
#[test]
fn une_base_saine_de_2000_noeuds_n_est_pas_dite_tronquee() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("saine");
    base_de(&chemin, 2000);
    let s = SubstrateStore::open_read_only(&chemin).expect("base saine refusée");
    assert_eq!(s.node_count(), 2000);
}

/// (g) `out_degree(NodeId(1)) == 0` n'est pas une régression : les arêtes de la
/// fabrication pointent du nœud vers le PRÉCÉDENT, donc le premier nœud n'a aucune
/// arête sortante et une arête entrante.
#[test]
fn les_degres_de_la_chaine_sont_ceux_de_la_fabrication() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("chaine");
    base_de(&chemin, 50);
    let s = SubstrateStore::open_read_only(&chemin).unwrap();
    let ids = s.node_ids();
    let (premier, second, dernier) = (ids[0], ids[1], ids[ids.len() - 1]);
    assert_eq!(
        s.out_degree(premier),
        0,
        "le premier nœud n'a pas d'arête sortante"
    );
    assert_eq!(
        s.in_degree(premier),
        1,
        "…et une arête entrante (du second)"
    );
    assert_eq!(s.out_degree(second), 1);
    assert_eq!(s.out_degree(dernier), 1);
    assert_eq!(s.in_degree(dernier), 0);
}
