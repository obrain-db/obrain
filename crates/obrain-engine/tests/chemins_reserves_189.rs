//! Clés réservées sur éléments DISTANTS d'un chemin nommé (#189, revue indépendante).
//!
//! Deux branches d'erreur du VLE n'étaient couvertes par aucun test :
//!   1. une ARÊTE distante portant une clé réservée (`_type`, `_id`, `_base`,
//!      `_source`, `_target`) fait échouer `valeur_arete` ;
//!   2. un NŒUD distant portant une clé réservée, **intermédiaire** d'un segment
//!      à longueur variable, fait échouer `valeur_noeud` — la cible finale étant
//!      saine, le contrôle de la colonne de cible ne court-circuite pas ce cas.
//!
//! Les deux refusent au lieu de servir une métadonnée falsifiée ou d'écraser la
//! valeur utilisateur : aucune valeur fausse ne doit sortir.
//!
//! ```bash
//! cargo test -p obrain-engine --features "gql,cypher" --test chemins_reserves_189
//! ```
#![cfg(all(feature = "gql", feature = "cypher"))]

use obrain_engine::ObrainDB;

/// alpha (primaire) : Note n -[:L]-> Hub h -[:T]-> ref(beta, doc_1) ;
/// beta : doc_1 -[:Q {_type:'forged'}]-> doc_2,
///        doc_1 -[:S]-> hop (_id: 7) -[:S]-> doc_3 (propre).
///
/// `hop` est intermédiaire du segment `[:L|T|S*3..3]`, `doc_3` en est la cible.
fn bases() -> (ObrainDB, ObrainDB) {
    let beta = ObrainDB::new_in_memory();
    let sb = beta.session();
    sb.execute("INSERT (:Doc {id: 'doc_1', name: 'un'})")
        .expect("doc_1");
    sb.execute("INSERT (:Doc {id: 'doc_2', name: 'deux'})")
        .expect("doc_2");
    sb.execute("INSERT (:Doc {id: 'doc_3', name: 'trois'})")
        .expect("doc_3");
    // Clé réservée d'ARÊTE, écrite par l'utilisateur : acceptée à l'écriture,
    // refusée à l'hydratation.
    sb.execute(
        "MATCH (a:Doc {id: 'doc_1'}), (b:Doc {id: 'doc_2'}) CREATE (a)-[:Q {_type: 'forged'}]->(b)",
    )
    .expect("arete Q reservee");
    // Nœud portant une clé réservée de NŒUD, intermédiaire d'un segment.
    sb.execute("INSERT (:Hop {id: 'hop', name: 'saut', _id: 7})")
        .expect("hop reserve");
    sb.execute("MATCH (a:Doc {id: 'doc_1'}), (h:Hop) CREATE (a)-[:S]->(h)")
        .expect("S entrant");
    sb.execute("MATCH (h:Hop), (c:Doc {id: 'doc_3'}) CREATE (h)-[:S]->(c)")
        .expect("S sortant");

    let alpha = ObrainDB::new_in_memory();
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("note");
    sa.execute("INSERT (:Hub {name: 'h'})").expect("hub");
    sa.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L]->(h)")
        .expect("L");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:T]->(ref(\"beta\", \"doc_1\"))")
        .expect("T");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach beta");
    (alpha, beta)
}

/// L'arête `Q` de beta porte `_type` : la traverser dans un chemin nommé doit
/// ÉCHOUER explicitement, pas rendre la métadonnée ni la valeur utilisateur.
#[test]
fn arete_distante_a_cle_reservee_refusee() {
    let (alpha, _beta) = bases();
    let r = alpha.session().execute(
        "MATCH WALK p = (n:Note)-[:L|T|Q*3..3]->(z) RETURN [r IN relationships(p) | type(r)]",
    );
    let msg = match r {
        Ok(v) => panic!("une arête distante à clé réservée doit être refusée, obtenu {v:?}"),
        Err(e) => format!("{e}"),
    };
    assert!(
        msg.contains("réservée"),
        "le message doit nommer la clé réservée : {msg}"
    );
    assert!(
        msg.contains("arête"),
        "le message doit désigner une arête : {msg}"
    );
}

/// `hop` porte `_id` et n'est pas la cible du segment : c'est `valeur_noeud`, sur
/// les nœuds du chemin, qui doit refuser — le contrôle de la colonne de cible ne
/// voit que `doc_3`, qui est sain.
#[test]
fn noeud_distant_reserve_intermediaire_refuse() {
    let (alpha, _beta) = bases();
    let r = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L|T|S*4..4]->(z) RETURN [x IN nodes(p) | x.name]");
    let msg = match r {
        Ok(v) => panic!("un nœud distant intermédiaire réservé doit être refusé, obtenu {v:?}"),
        Err(e) => format!("{e}"),
    };
    assert!(
        msg.contains("réservée"),
        "le message doit nommer la clé réservée : {msg}"
    );
    // Le message doit venir de la matérialisation du CHEMIN et non du contrôle de
    // la colonne de cible : `hop` est intermédiaire, la cible `doc_3` est saine.
    assert!(
        msg.contains("chemin refusé") && msg.contains("porte la propriété"),
        "le refus doit venir des nœuds du chemin, pas de la colonne de cible : {msg}"
    );
}

/// Témoin : le même chemin SANS la clé réservée doit rendre les noms exacts.
/// Sans ce témoin, un refus dû à une erreur de fixture passerait pour un succès.
#[test]
fn temoin_meme_chemin_sans_cle_reservee_rend_les_noms() {
    let (alpha, beta) = bases();
    // On retire la propriété réservée de `hop` DEPUIS SA BASE : la session primaire
    // refuse déjà d'hydrater un nœud distant réservé, donc elle ne peut pas non
    // plus le réparer.
    beta.session()
        .execute("MATCH (h:Hop) REMOVE h._id")
        .expect("remove depuis beta");
    let r = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L|T|S*4..4]->(z) RETURN [x IN nodes(p) | x.name]")
        .expect("chemin sain");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    let noms: Vec<Option<String>> = match &r.rows[0][0] {
        obrain_common::types::Value::List(items) => items
            .iter()
            .map(|v| match v {
                obrain_common::types::Value::String(s) => Some(s.to_string()),
                obrain_common::types::Value::Null => None,
                autre => panic!("texte attendu : {autre:?}"),
            })
            .collect(),
        autre => panic!("liste attendue : {autre:?}"),
    };
    assert_eq!(
        noms,
        vec![
            Some("n".to_string()),
            Some("h".to_string()),
            Some("un".to_string()),
            Some("saut".to_string()),
            Some("trois".to_string()),
        ],
        "le chemin primaire -> beta -> hop -> doc_3"
    );
}
