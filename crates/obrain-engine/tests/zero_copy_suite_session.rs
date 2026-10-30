//! Câblage Session de la SUITE du pont zero-copy (chantier VIEWS-suite) :
//! `Session::for_each_prop_view`, `Session::edge_prop_str_view`,
//! `Session::get_edge_property` et `Session::collect_props_into` doivent
//! déléguer au substrate — la sémantique fine (LWW, sources, gardes) est
//! couverte par `obrain-substrate/tests/zero_copy_suite.rs` ; ici on
//! vérifie la délégation et le comportement sur base NON-substrate.

use std::ops::ControlFlow;

use obrain_common::types::{EdgeId, Value};
use obrain_engine::{ObrainDB, ResultBuf, StrViewSource};

#[test]
fn session_balayage_vues_arete_et_arene() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).expect("open_substrate");
    let session = db.session();

    let long = "contexte long vers la colonne blob du substrate ".repeat(7); // ~340 o
    for i in 0..8 {
        session
            .execute(&format!("INSERT (:Fait {{fid: {i}, texte: 'fait numero {i}'}})"))
            .expect("insert nœud");
    }
    session
        .execute("INSERT (:Src {s: 1})-[:CITE {contexte: 'court en zone'}]->(:Dst {d: 1})")
        .expect("insert arête courte");
    session
        .execute(&format!(
            "INSERT (:Src {{s: 2}})-[:CITE {{contexte: '{long}'}}]->(:Dst {{d: 2}})"
        ))
        .expect("insert arête longue");

    // Balayage : 8 nœuds :Fait servis, texte conforme.
    let mut vus = Vec::new();
    let servies = session
        .for_each_prop_view(Some("Fait"), "texte", |id, s| {
            vus.push((id.0, s.to_owned()));
            ControlFlow::Continue(())
        })
        .expect("balayage substrate");
    assert_eq!(servies, 8);
    vus.sort();
    for (i, (_, texte)) in vus.iter().enumerate() {
        assert_eq!(texte, &format!("fait numero {i}"));
    }

    // Vues d'arêtes : les deux sources, égalité avec le chemin possédé.
    let mut sources = Vec::new();
    for i in 0..64u64 {
        let id = EdgeId(i);
        if let Some(vue) = session.edge_prop_str_view(id, "contexte") {
            let possedee = session.get_edge_property(id, "contexte");
            assert_eq!(
                possedee,
                Some(Value::String(vue.as_str().into())),
                "la vue d'arête dit ce que dit le chemin possédé"
            );
            sources.push(vue.source());
        }
    }
    sources.sort_by_key(|s| format!("{s:?}"));
    assert_eq!(
        sources,
        vec![StrViewSource::Blob, StrViewSource::Zone],
        "une arête en colonne blob, une en PropsZone"
    );

    // Arène : collecte == balayage, et clear() amorti entre requêtes.
    let mut buf = ResultBuf::new();
    let n = session
        .collect_props_into(Some("Fait"), "texte", &mut buf)
        .expect("collecte");
    assert_eq!(n, 8);
    assert_eq!(buf.len(), 8);
    let capa = buf.capacite_octets();
    let n2 = session
        .collect_props_into(Some("Fait"), "texte", &mut buf)
        .expect("collecte 2");
    assert_eq!(n2, 8);
    assert_eq!(buf.capacite_octets(), capa, "arène amortie, zéro realloc");
}

#[test]
fn session_non_substrate_rend_zero_et_none() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let servies = session
        .for_each_prop_view(None, "texte", |_, _| panic!("aucun appel attendu"))
        .expect("Ok(0) sur base mémoire");
    assert_eq!(servies, 0);
    assert!(session.edge_prop_str_view(EdgeId(1), "contexte").is_none());
}
