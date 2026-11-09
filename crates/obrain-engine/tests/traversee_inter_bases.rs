//! Aretes inter-bases V2 — traversee multi-store : frontiere d'expansion,
//! base(n), WHERE a cheval, cycle A->B->A, cible supprimee = absent.
//!
//! ```bash
//! cargo test -p obrain-engine --features "gql,wal" --test traversee_inter_bases
//! ```

#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Deux bases en memoire, attachees : alpha (primaire) -> beta.
/// beta porte un Doc {id: "doc_1", titre: "Guide"}.
fn deux_bases() -> (ObrainDB, ObrainDB) {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();

    beta.session()
        .execute("INSERT (:Doc {id: 'doc_1', titre: 'Guide'})")
        .expect("insert beta");
    // Index de propriete sur la clef de resolution : seek O(1) (T17b).
    beta.graph_store().create_property_index("id");

    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'note agent'})")
        .expect("insert alpha");
    sa.execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"doc_1\"))")
        .expect("create arete externe");

    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach beta");
    (alpha, beta)
}

#[test]
fn la_traversee_franchit_la_frontiere_et_lit_les_proprietes_distantes() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN a.name, b.titre")
        .expect("traversee");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("note agent".into()));
    assert_eq!(
        r.rows[0][1],
        Value::String("Guide".into()),
        "la propriete du noeud distant se lit via son store (hydratation frontiere)"
    );
}

#[test]
fn base_de_n_rend_le_slug_du_store_du_noeud() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN base(a), base(b)")
        .expect("base()");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("alpha".into()));
    assert_eq!(r.rows[0][1], Value::String("beta".into()));
}

#[test]
fn where_a_cheval_sur_les_deux_bases() {
    let (alpha, _beta) = deux_bases();
    let session = alpha.session();
    let r = session
        .execute(
            "MATCH (a:Note)-[:DOCUMENTS]->(b) \
             WHERE b.titre = 'Guide' AND a.name = 'note agent' RETURN b.titre",
        )
        .expect("where");
    assert_eq!(r.rows.len(), 1);
    let r = session
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) WHERE b.titre = 'Autre' RETURN b")
        .expect("where negatif");
    assert_eq!(r.rows.len(), 0);
    // NOT a cheval : le predicat distant est faux -> NOT vrai.
    let r = session
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) WHERE NOT b.titre = 'Autre' RETURN b.titre")
        .expect("not");
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn le_type_d_arete_filtre_aussi_les_aretes_externes() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:AUTRE_TYPE]->(b) RETURN b")
        .expect("filtre type");
    assert_eq!(r.rows.len(), 0, "un type qui ne matche pas ne traverse pas");
}

#[test]
fn cible_supprimee_rend_absent_jamais_une_erreur() {
    let (alpha, beta) = deux_bases();
    beta.session()
        .execute("MATCH (d:Doc) DETACH DELETE d")
        .expect("suppression cible");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b")
        .expect("la ref pendante ne doit JAMAIS lever d'erreur");
    assert_eq!(r.rows.len(), 0, "cible disparue = ligne absente (trivalue)");
    // Le compteur de refs pendantes a note le passage (health).
    let reg = alpha.base_registry().expect("registre");
    assert!(
        reg.dangling_refs.load(std::sync::atomic::Ordering::Relaxed) > 0,
        "dangling_refs doit compter la resolution manquee"
    );
    // Et la note source existe toujours : le motif sans la partie distante vit.
    let r = alpha
        .session()
        .execute("MATCH (a:Note) RETURN a.name")
        .expect("source intacte");
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn sans_attache_la_reference_est_absente_pas_une_erreur() {
    let alpha = ObrainDB::new_in_memory();
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("insert");
    sa.execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"doc_1\"))")
        .expect("create");
    // AUCUN attach_bases : pas de registre, pas de resolution.
    let r = sa
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b")
        .expect("sans registre : absent, pas d'erreur");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn hote_distant_reste_absent_et_compte() {
    let (alpha, _beta) = deux_bases();
    alpha
        .session()
        .execute("MATCH (a:Note) CREATE (a)-[:MIRRORS]->(ref(\"dgx-2\", \"beta\", \"doc_1\"))")
        .expect("create multi-host");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:MIRRORS]->(b) RETURN b")
        .expect("hote distant : absent");
    assert_eq!(r.rows.len(), 0, "resolution reseau reservee (V5)");
    let reg = alpha.base_registry().expect("registre");
    assert!(
        reg.remote_refs_unresolved
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    );
}

#[test]
fn le_motif_continue_dans_le_store_cible() {
    // alpha -> beta (externe), puis une arete INTERNE de beta.
    let (alpha, beta) = deux_bases();
    beta.session()
        .execute("MATCH (d:Doc) CREATE (d)-[:CITE]->(:Source {id: 'src_1', nom: 'Manuel'})")
        .expect("arete interne beta");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b)-[:CITE]->(c) RETURN c.nom, base(c)")
        .expect("suite du motif dans le store cible");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("Manuel".into()));
    assert_eq!(r.rows[0][1], Value::String("beta".into()));
}

#[test]
fn cycle_a_b_a_revient_sur_la_base_primaire_en_natif() {
    let (alpha, beta) = deux_bases();
    // beta -> alpha : le Doc pointe la Note par sa clef numerique.
    let note_id = { alpha.graph_store().nodes_by_label("Note")[0] };
    beta.attach_bases("beta", None);
    beta.session()
        .execute(&format!(
            "MATCH (d:Doc) CREATE (d)-[:ANNOTE]->(ref(\"alpha\", \"{}\"))",
            note_id.0
        ))
        .expect("create beta -> alpha");

    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b)-[:ANNOTE]->(c) RETURN base(c), c.name")
        .expect("cycle A -> B -> A");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(
        r.rows[0][0],
        Value::String("alpha".into()),
        "le retour au primaire redevient un noeud natif"
    );
    assert_eq!(r.rows[0][1], Value::String("note agent".into()));
    // La semantique de visite tient : le motif est borne, pas de boucle.
}

#[test]
fn clef_numerique_resout_par_id_de_noeud() {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    beta.session()
        .execute("INSERT (:Doc {titre: 'Brut'})")
        .expect("insert");
    let doc_id = { beta.graph_store().nodes_by_label("Doc")[0] };
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("insert");
    sa.execute(&format!(
        "MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"{}\"))",
        doc_id.0
    ))
    .expect("create clef numerique");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre")
        .expect("traversee");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("Brut".into()));
}

#[test]
fn count_sur_deux_sauts_traverse_les_deux_bases() {
    // Verifie aussi le garde-fou du chemin factorise (aggregat multi-sauts).
    let (alpha, beta) = deux_bases();
    beta.session()
        .execute("MATCH (d:Doc) CREATE (d)-[:CITE]->(:Source {id: 'src_1'})")
        .expect("arete interne beta");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b)-[:CITE]->(c) RETURN count(c)")
        .expect("count");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::Int64(1));
}

#[test]
fn return_du_noeud_distant_rend_une_map_avec_sa_base() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b")
        .expect("return noeud distant");
    assert_eq!(r.rows.len(), 1);
    match &r.rows[0][0] {
        Value::Map(m) => {
            use obrain_common::types::PropertyKey;
            assert_eq!(
                m.get(&PropertyKey::new("_base")),
                Some(&Value::String("beta".into()))
            );
            assert_eq!(
                m.get(&PropertyKey::new("titre")),
                Some(&Value::String("Guide".into()))
            );
        }
        autre => panic!("attendu une Map de noeud distant, recu {autre:?}"),
    }
}

/// Durabilite + traversee : les aretes externes ecrites puis la base
/// rouverte traversent encore (le registre se re-attache).
#[cfg(feature = "wal")]
#[test]
fn apres_reouverture_la_traversee_fonctionne_encore() {
    let dir = tempfile::tempdir().expect("tempdir");
    let chemin_alpha = dir.path().join("alpha");
    let chemin_beta = dir.path().join("beta");
    {
        let beta = ObrainDB::open(&chemin_beta).expect("open beta");
        beta.session()
            .execute("INSERT (:Doc {id: 'doc_1', titre: 'Guide'})")
            .expect("insert");
        beta.close().expect("close");
    }
    {
        let alpha = ObrainDB::open(&chemin_alpha).expect("open alpha");
        let sa = alpha.session();
        sa.execute("INSERT (:Note {name: 'n'})").expect("insert");
        sa.execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"doc_1\"))")
            .expect("create");
        alpha.close().expect("close");
    }
    // Reouverture : attache par CHEMIN (ouverture paresseuse).
    let alpha = ObrainDB::open(&chemin_alpha).expect("reopen alpha");
    alpha.attach_bases("alpha", Some(dir.path()));
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre")
        .expect("traversee apres reouverture");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("Guide".into()));
}

#[test]
fn label_sur_le_noeud_distant_filtre_via_ses_labels_embarques() {
    let (alpha, _beta) = deux_bases();
    let session = alpha.session();
    let r = session
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b:Doc) RETURN b.titre")
        .expect("label distant");
    assert_eq!(r.rows.len(), 1, "le label du noeud distant doit matcher");
    let r = session
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b:Autre) RETURN b")
        .expect("label distant negatif");
    assert_eq!(r.rows.len(), 0);
    let r = session
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN labels(b)")
        .expect("labels()");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(
        r.rows[0][0],
        Value::List(vec![Value::String("Doc".into())].into())
    );
}

#[test]
fn motif_qualifie_in_slug_restreint_a_la_base() {
    let (alpha, _beta) = deux_bases();
    let session = alpha.session();
    let r = session
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b IN \"beta\") RETURN b.titre")
        .expect("IN slug");
    assert_eq!(r.rows.len(), 1);
    let r = session
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b IN \"gamma\") RETURN b")
        .expect("IN mauvaise base");
    assert_eq!(r.rows.len(), 0);
    // Qualifieur + label.
    let r = session
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b IN \"beta\":Doc) RETURN b.titre")
        .expect("IN slug + label");
    assert_eq!(r.rows.len(), 1);
    // Cote source : la base primaire se nomme.
    let r = session
        .execute("MATCH (a IN \"alpha\":Note)-[:DOCUMENTS]->(b) RETURN a.name")
        .expect("IN source");
    assert_eq!(r.rows.len(), 1);
    // En ecriture, IN est refuse avec un message enseignant.
    let err = session
        .execute("CREATE (x IN \"beta\":Doc {id: \x27k\x27})")
        .expect_err("IN en ecriture refuse");
    assert!(format!("{err}").contains("LECTURE"));
}

// ─── V4 : traversee multi-bases a profondeur variable ────────────────────

/// Trois bases : alpha -> beta -> gamma par aretes externes du meme type.
/// Le motif *2..2 traverse DEUX frontieres.
#[test]
fn profondeur_variable_traverse_deux_frontieres() {
    let (alpha, beta) = deux_bases();
    let gamma = ObrainDB::new_in_memory();
    gamma
        .session()
        .execute("INSERT (:Src {id: \x27src_1\x27, nom: \x27Manuel\x27})")
        .expect("insert gamma");
    gamma.graph_store().create_property_index("id");
    beta.attach_bases("beta", None);
    beta.session()
        .execute("MATCH (d:Doc) CREATE (d)-[:DOCUMENTS]->(ref(\"gamma\", \"src_1\"))")
        .expect("create beta -> gamma");
    alpha.attach_base("gamma", &gamma).expect("attach gamma");

    let s = alpha.session();
    let r = s
        .execute("MATCH (a:Note)-[:DOCUMENTS*2..2]->(c) RETURN c.nom, base(c)")
        .expect("vle deux frontieres");
    assert_eq!(r.rows.len(), 1, "A->B->C en deux sauts externes");
    assert_eq!(r.rows[0][0], Value::String("Manuel".into()));
    assert_eq!(r.rows[0][1], Value::String("gamma".into()));
    // *1..2 voit b (saut 1) ET c (saut 2).
    let r = s
        .execute("MATCH (a:Note)-[:DOCUMENTS*1..2]->(x) RETURN x")
        .expect("vle 1..2");
    assert_eq!(r.rows.len(), 2);
}

/// Cycle A->B->A par le motif de longueur variable : le retour redevient
/// un noeud natif de la base primaire.
#[test]
fn profondeur_variable_cycle_a_b_a() {
    let (alpha, beta) = deux_bases();
    let note_id = { alpha.graph_store().nodes_by_label("Note")[0] };
    beta.attach_bases("beta", None);
    beta.session()
        .execute(&format!(
            "MATCH (d:Doc) CREATE (d)-[:DOCUMENTS]->(ref(\"alpha\", \"{}\"))",
            note_id.0
        ))
        .expect("create beta -> alpha");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS*2..2]->(c) RETURN c.name, base(c)")
        .expect("cycle vle");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("note agent".into()));
    assert_eq!(r.rows[0][1], Value::String("alpha".into()));
}

/// Droits refuses au DEUXIEME saut : la branche est coupee sans oracle —
/// le premier saut vit, le second est absent, jamais une erreur.
#[test]
fn profondeur_variable_droits_refuses_au_deuxieme_saut() {
    use obrain_core::execution::AccessContext;
    use std::sync::Arc;
    let (alpha, beta) = deux_bases();
    let gamma = ObrainDB::new_in_memory();
    gamma
        .session()
        .execute("INSERT (:Src {id: \x27src_1\x27})")
        .expect("insert");
    beta.attach_bases("beta", None);
    beta.session()
        .execute("MATCH (d:Doc) CREATE (d)-[:DOCUMENTS]->(ref(\"gamma\", \"src_1\"))")
        .expect("create");
    alpha.attach_base("gamma", &gamma).expect("attach");

    let s = alpha.session();
    s.set_access_context(Arc::new(AccessContext::new(
        "demi",
        Box::new(|_, slug| slug == "alpha" || slug == "beta"),
    )));
    let r = s
        .execute("MATCH (a:Note)-[:DOCUMENTS*1..2]->(x) RETURN x")
        .expect("jamais d erreur");
    assert_eq!(
        r.rows.len(),
        1,
        "saut 1 (beta) autorise, saut 2 (gamma) refuse : branche coupee"
    );
    let reg = alpha.base_registry().expect("registre");
    assert!(
        reg.access_denied_refs
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    );
}

/// Mode ACYCLIC multi-bases : la clef de visite est (base, id) — deux
/// NodeId identiques de deux bases ne se confondent pas.
#[test]
fn profondeur_variable_acyclic_ne_confond_pas_les_bases() {
    // alpha note NodeId(1) -> beta doc NodeId(1) : avec une clef nue, le
    // mode ACYCLIC croirait revisiter le meme noeud et couperait.
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH ACYCLIC (a:Note)-[:DOCUMENTS*1..2]->(x) RETURN x")
        .expect("acyclic");
    assert_eq!(
        r.rows.len(),
        1,
        "NodeId(1) d alpha et NodeId(1) de beta sont deux noeuds distincts"
    );
}
