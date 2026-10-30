//! Tests de la SUITE du pont mémoire zero-copy (chantier VIEWS-suite) :
//! vues sur les propriétés d'ARÊTES (`edge_prop_str_view`), balayage
//! callback `for_each_node_prop_str_view` (une garde par zone,
//! `ControlFlow`), et `ResultBuf` (arène réutilisée, `clear()` sans
//! dealloc). Mêmes exigences que les tests O1 (`zero_copy_views.rs`) :
//! validité sous garde, LWW/tombstone, entités mortes, durabilité de la
//! représentation à la réouverture.
//!
//! Le refus d'échappement de l'emprunt du callback est un doc-test
//! `compile_fail` sur `SubstrateStore::for_each_node_prop_str_view`
//! (le borrow checker est le test), comme le refus hors-garde d'O1.

use std::collections::BTreeMap;
use std::ops::ControlFlow;

use obrain_common::types::{EdgeId, NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::result_buf::ResultBuf;
use obrain_substrate::store::SubstrateStore;
use obrain_substrate::view::StrViewSource;

// ---------------------------------------------------------------------------
// Vues sur les propriétés d'ARÊTES — les trois chemins, LWW, vivacité
// ---------------------------------------------------------------------------

#[test]
fn vue_arete_zone_egale_valeur_possedee() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let a = s.create_node(&["Fait"]);
    let b = s.create_node(&["Fait"]);
    let e = s.create_edge(a, b, "CITE");
    s.set_edge_property(
        e,
        "contexte",
        Value::String("cité pendant le seeding".into()),
    );

    let vue = s.edge_prop_str_view(e, "contexte").expect("vue présente");
    assert_eq!(&*vue, "cité pendant le seeding");
    assert_eq!(
        vue.source(),
        StrViewSource::Zone,
        "une base neuve écrit la chaîne v2 de l'arête : la vue doit emprunter au heap mmap"
    );

    // La vue dit EXACTEMENT ce que dit le chemin possédé.
    let possedee = s.get_edge_property(e, &PropertyKey::new("contexte"));
    assert_eq!(
        possedee,
        Some(Value::String("cité pendant le seeding".into()))
    );
}

#[test]
fn vue_arete_blob_pour_plus_de_256_octets() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let a = s.create_node(&["Fait"]);
    let b = s.create_node(&["Fait"]);
    let e = s.create_edge(a, b, "CITE");
    let long = "contexte-d-arete-au-dela-du-seuil-blob ".repeat(10); // ~390 o
    assert!(long.len() > 256);
    s.set_edge_property(e, "contexte", Value::String(long.as_str().into()));

    let vue = s.edge_prop_str_view(e, "contexte").expect("vue présente");
    assert_eq!(&*vue, long.as_str());
    assert_eq!(
        vue.source(),
        StrViewSource::Blob,
        "au-delà de 256 o la valeur d'arête vit en colonne blob (EntityKind::Edge)"
    );
    // Deux accès successifs re-dérivent le même slice (bornes + UTF-8 revalidés).
    assert_eq!(vue.as_str(), vue.as_str());
}

#[test]
fn arete_lww_tombstone_et_arete_morte() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let a = s.create_node(&["Fait"]);
    let b = s.create_node(&["Fait"]);

    // LWW : la vue voit la DERNIÈRE valeur écrite.
    let e = s.create_edge(a, b, "CITE");
    s.set_edge_property(e, "contexte", Value::String("v1".into()));
    s.set_edge_property(e, "contexte", Value::String("v2 — corrigée".into()));
    assert_eq!(
        &*s.edge_prop_str_view(e, "contexte").unwrap(),
        "v2 — corrigée"
    );

    // Tombstone : la propriété retirée est invisible à la vue.
    s.remove_edge_property(e, "contexte");
    assert!(
        s.edge_prop_str_view(e, "contexte").is_none(),
        "la tombstone doit masquer la valeur d'arête"
    );

    // Arête morte : aucune vue, même avec une valeur encore sur disque.
    let f = s.create_edge(a, b, "CITE");
    s.set_edge_property(f, "contexte", Value::String("condamnée".into()));
    assert!(s.delete_edge(f));
    assert!(s.edge_prop_str_view(f, "contexte").is_none());
}

#[test]
fn vue_arete_non_string_cle_et_arete_absentes() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let a = s.create_node(&["Fait"]);
    let b = s.create_node(&["Fait"]);
    let e = s.create_edge(a, b, "CITE");
    s.set_edge_property(e, "poids", Value::Int64(42));
    assert!(
        s.edge_prop_str_view(e, "poids").is_none(),
        "une vue str ne se pose pas sur un entier"
    );
    assert!(s.edge_prop_str_view(e, "absente").is_none());
    assert!(s.edge_prop_str_view(EdgeId(999_999), "contexte").is_none());
}

#[test]
fn vue_arete_survit_a_la_reouverture() {
    // La représentation DISQUE (formats v2 little-endian, goldens O1) est
    // relue telle quelle : flush, réouverture en lecture seule, mêmes vues.
    let tmp = tempfile::tempdir().unwrap();
    let chemin = tmp.path(); // le répertoire EST le substrate
    let long = "persistance-du-contexte-d-arete ".repeat(12); // ~384 o → blob
    {
        let s = SubstrateStore::create(chemin).unwrap();
        let a = s.create_node(&["Fait"]);
        let b = s.create_node(&["Fait"]);
        let e1 = s.create_edge(a, b, "CITE");
        s.set_edge_property(e1, "contexte", Value::String("court en zone".into()));
        let e2 = s.create_edge(b, a, "CITE");
        s.set_edge_property(e2, "contexte", Value::String(long.as_str().into()));
        s.flush().unwrap();
    }
    let s = SubstrateStore::open_read_only(chemin).unwrap();
    let mut vues: Vec<(String, StrViewSource)> = Vec::new();
    for i in 0..16u64 {
        if let Some(v) = s.edge_prop_str_view(EdgeId(i), "contexte") {
            vues.push((v.as_str().to_owned(), v.source()));
        }
    }
    vues.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(vues.len(), 2, "les deux arêtes doivent être relues");
    assert_eq!(vues[0], ("court en zone".into(), StrViewSource::Zone));
    assert_eq!(vues[1], (long, StrViewSource::Blob));
}

// ---------------------------------------------------------------------------
// Balayage callback — parité avec la boucle par nœud, filtre, Break
// ---------------------------------------------------------------------------

/// Store mixte : zone + blob + tombstone + non-String + nœud mort + autre
/// label. Rend le store et la table de référence `id → texte` bâtie par la
/// boucle `node_prop_str_view` PAR NŒUD (la sémantique de référence O1).
fn store_mixte() -> (SubstrateStore, BTreeMap<u64, String>) {
    let s = SubstrateStore::open_tempfile().unwrap();
    let long = "valeur-longue-vers-la-colonne-blob ".repeat(9); // ~315 o
    for i in 0..40 {
        let n = s.create_node(&["Fait"]);
        s.set_node_property(n, "texte", Value::String(format!("fait court {i}").into()));
    }
    for _ in 0..10 {
        let n = s.create_node(&["Fait"]);
        s.set_node_property(n, "texte", Value::String(long.as_str().into()));
    }
    for _ in 0..5 {
        // porteur retiré : tombstone LWW, invisible.
        let n = s.create_node(&["Fait"]);
        s.set_node_property(n, "texte", Value::String("éphémère".into()));
        s.remove_node_property(n, "texte");
    }
    for _ in 0..3 {
        // nœud mort : invisible.
        let n = s.create_node(&["Fait"]);
        s.set_node_property(n, "texte", Value::String("condamné".into()));
        s.delete_node(n);
    }
    for i in 0..4 {
        // autre label, même clé : visible sans filtre, exclu par le filtre.
        let n = s.create_node(&["Autre"]);
        s.set_node_property(n, "texte", Value::String(format!("autre {i}").into()));
    }
    for _ in 0..5 {
        // non-String : jamais servi.
        let n = s.create_node(&["Fait"]);
        s.set_node_property(n, "texte", Value::Int64(7));
    }
    let mut reference = BTreeMap::new();
    for id in s.node_ids() {
        if let Some(v) = s.node_prop_str_view(id, "texte") {
            reference.insert(id.0, v.as_str().to_owned());
        }
    }
    (s, reference)
}

#[test]
fn balayage_egal_boucle_par_noeud() {
    let (s, reference) = store_mixte();
    assert_eq!(reference.len(), 54, "40 zone + 10 blob + 4 autre-label");

    let mut vus: BTreeMap<u64, String> = BTreeMap::new();
    let servies = s
        .for_each_node_prop_str_view(None, "texte", |id, texte| {
            assert!(
                vus.insert(id.0, texte.to_owned()).is_none(),
                "chaque nœud est servi UNE fois"
            );
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(servies, reference.len() as u64);
    assert_eq!(
        vus, reference,
        "le balayage dit EXACTEMENT ce que dit la boucle par nœud"
    );
}

#[test]
fn balayage_filtre_par_label() {
    let (s, reference) = store_mixte();
    let mut vus: BTreeMap<u64, String> = BTreeMap::new();
    let servies = s
        .for_each_node_prop_str_view(Some("Fait"), "texte", |id, texte| {
            vus.insert(id.0, texte.to_owned());
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(servies, 50, "40 zone + 10 blob, sans les `Autre`");
    for (id, texte) in &vus {
        assert_eq!(reference.get(id), Some(texte));
        assert!(
            !texte.starts_with("autre"),
            "le filtre exclut l'autre label"
        );
    }
    // Label jamais interné : zéro nœud, zéro appel.
    let servies = s
        .for_each_node_prop_str_view(Some("Inconnu"), "texte", |_, _| {
            panic!("aucun appel attendu")
        })
        .unwrap();
    assert_eq!(servies, 0);
}

#[test]
fn balayage_break_arrete_net() {
    let (s, _) = store_mixte();
    let mut appels = 0u64;
    let servies = s
        .for_each_node_prop_str_view(None, "texte", |_, _| {
            appels += 1;
            if appels == 3 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .unwrap();
    assert_eq!(appels, 3, "Break arrête après le 3e appel");
    assert_eq!(servies, 3, "le compte rendu s'arrête avec le Break");
}

#[test]
fn balayage_survit_a_la_reouverture() {
    // Même exigence de durabilité que les vues : le balayage relit la
    // représentation disque après flush + réouverture lecture seule.
    let tmp = tempfile::tempdir().unwrap();
    let chemin = tmp.path(); // le répertoire EST le substrate
    let attendu: u64 = 12;
    {
        let s = SubstrateStore::create(chemin).unwrap();
        for i in 0..attendu {
            let n = s.create_node(&["Fait"]);
            s.set_node_property(n, "texte", Value::String(format!("persisté {i}").into()));
        }
        s.flush().unwrap();
    }
    let s = SubstrateStore::open_read_only(chemin).unwrap();
    let mut total_octets = 0usize;
    let servies = s
        .for_each_node_prop_str_view(Some("Fait"), "texte", |_, texte| {
            total_octets += texte.len();
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(servies, attendu);
    assert!(total_octets > 0);
}

// ---------------------------------------------------------------------------
// ResultBuf — arène amortie, clear() sans dealloc
// ---------------------------------------------------------------------------

#[test]
fn resultbuf_contenu_et_clear_sans_dealloc() {
    let mut buf = ResultBuf::new();
    buf.push(3, "alpha");
    buf.push(5, "béta");
    assert_eq!(buf.len(), 2);
    assert_eq!(buf.octets(), "alpha".len() + "béta".len());
    assert_eq!(buf.get(0), Some((3, "alpha")));
    assert_eq!(buf.get(1), Some((5, "béta")));
    assert_eq!(buf.get(2), None);
    assert_eq!(
        buf.iter().collect::<Vec<_>>(),
        vec![(3, "alpha"), (5, "béta")]
    );

    let (co, cl) = (buf.capacite_octets(), buf.capacite_lignes());
    buf.clear();
    assert!(buf.is_empty());
    assert_eq!(buf.octets(), 0);
    assert_eq!(buf.capacite_octets(), co, "clear() ne rend pas l'arène");
    assert_eq!(buf.capacite_lignes(), cl, "clear() ne rend pas les lignes");

    // Réutilisation : mêmes données, AUCUNE croissance de capacité.
    buf.push(3, "alpha");
    buf.push(5, "béta");
    assert_eq!(buf.capacite_octets(), co);
    assert_eq!(buf.capacite_lignes(), cl);
}

#[test]
fn collect_into_egal_balayage_et_amortit() {
    let (s, reference) = store_mixte();
    let mut buf = ResultBuf::new();
    let servies = s
        .collect_node_prop_strs_into(None, "texte", &mut buf)
        .unwrap();
    assert_eq!(servies, reference.len() as u64);
    let collecte: BTreeMap<u64, String> = buf
        .iter()
        .map(|(id, texte)| (id, texte.to_owned()))
        .collect();
    assert_eq!(collecte, reference, "l'arène matérialise le même contenu");

    // Une deuxième collecte identique n'agrandit rien : l'arène est amortie.
    let (co, cl) = (buf.capacite_octets(), buf.capacite_lignes());
    let servies2 = s
        .collect_node_prop_strs_into(None, "texte", &mut buf)
        .unwrap();
    assert_eq!(servies2, servies);
    assert_eq!(buf.capacite_octets(), co, "aucune réallocation d'arène");
    assert_eq!(buf.capacite_lignes(), cl, "aucune réallocation de lignes");
}
