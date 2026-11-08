//! LE FILTRE SANS INDEX NE CHANGE PAS CE QUE LE MVCC MONTRE — contrat.
//!
//! ── CE QUI EST PROTÉGÉ (2026-11-08, C1 du plan performance) ────────────
//!
//! `MATCH (n:Label {prop: val})` sans index sur `prop` est planifié par la
//! branche « label d'abord » de `try_plan_filter_with_property_index`
//! (`crates/obrain-engine/src/query/planner/lpg/filter.rs`) : la liste des
//! nœuds du label est matérialisée et la propriété testée nœud par nœud à la
//! PLANIFICATION. Jusqu'à C1, ce test hydratait le nœud complet
//! (`get_node_at_epoch` / `get_node_versioned`). C1 le remplace par une
//! lecture ciblée d'UNE propriété (`get_node_property_batch_at_epoch` /
//! `..._versioned`) dont le contrat est l'IDENTITÉ de visibilité et de valeur
//! avec l'accès nœud qu'elle remplace.
//!
//! Ce fichier mesure ce contrat de deux manières, pour chaque scénario :
//!
//! 1. l'ORACLE D'HYDRATATION : la formule exacte de l'ancien chemin, calculée
//!    sur le store brut — `get_node_at_epoch(n, e).get_property(p) == v`
//!    recoupé par `is_node_visible_at_epoch(n, e)` (le second passage que le
//!    planificateur applique toujours) ;
//! 2. le TÉMOIN GÉNÉRIQUE : la même requête SANS label (`MATCH (n) WHERE
//!    n.p = v`), qui n'emprunte pas cette branche et applique le prédicat à
//!    l'EXÉCUTION, dans le même contexte transactionnel. C'est le seul oracle
//!    disponible pour le lecteur EN transaction (l'identifiant de transaction
//!    n'est pas exposé par la session).
//!
//! Les trois scénarios : (a) propriété modifiée par une transaction non
//! commitée, lue à l'époque courante et à une époque antérieure ; (b) nœud
//! supprimé par une transaction non commitée, cherché à l'ancienne et à la
//! nouvelle époque ; (c) trois écritures LWW de la même clef.
//!
//! Ce que ces tests ne PROMETTENT PAS : que l'époque antérieure rende
//! l'ANCIENNE VALEUR d'une propriété modifiée. Le substrat ne garde qu'UNE
//! version des propriétés (« Properties reflect the current state (not
//! versioned per-epoch) », `database/crud.rs`) ; la question est posée en
//! clair dans `(a)` et tranchée par l'oracle, pas par un chiffre attendu.
//!
//! MESURÉ AVANT C1 (filter.rs d'origine, 2026-11-08) : (a), (b'), (c) verts ;
//! (b) a révélé un écart ANTÉRIEUR à C1 — `nodes_by_label`, comme le
//! balayage complet, ne propose pas les nœuds tombstonés, donc un nœud
//! supprimé n'est retrouvé par AUCUN balayage à l'époque antérieure à sa
//! mort, alors que `get_node_at_epoch` le rend. (b) consigne cet écart tel
//! quel ; il est identique avant et après C1.
//!
//!   cargo test -p obrain-engine --test filtre_sans_index_mvcc

use obrain_common::types::{EpochId, NodeId, Value};
use obrain_engine::ObrainDB;
use obrain_engine::Session;

/// Valeurs d'une colonne entière, triées — la forme comparable d'un résultat.
fn entiers(session: &Session, q: &str) -> Vec<i64> {
    entiers_a(session, q, None)
}

fn entiers_a(session: &Session, q: &str, epoque: Option<EpochId>) -> Vec<i64> {
    let r = match epoque {
        Some(e) => session.execute_at_epoch(q, e),
        None => session.execute(q),
    }
    .unwrap_or_else(|e| panic!("requête refusée : {q}\n{e}"));
    let mut v: Vec<i64> = r
        .rows
        .into_iter()
        .map(|l| match &l[0] {
            Value::Int64(n) => *n,
            autre => panic!("entier attendu, reçu {autre:?} pour {q}"),
        })
        .collect();
    v.sort_unstable();
    v
}

/// L'ORACLE D'HYDRATATION pour un lecteur SANS transaction à l'époque `e` :
/// la formule de l'ancien chemin, sur le store brut. Rend les `id` (propriété
/// entière `id`) des nœuds de `candidats` dont la propriété `prop` vaut
/// `val`, visibles à `e`.
fn oracle_hydratation(
    db: &ObrainDB,
    candidats: &[NodeId],
    prop: &str,
    val: &Value,
    e: EpochId,
) -> Vec<i64> {
    let store = db.store();
    let mut v: Vec<i64> = candidats
        .iter()
        .filter_map(|&n| {
            let noeud = store.get_node_at_epoch(n, e)?;
            if noeud.get_property(prop) != Some(val) {
                return None;
            }
            if !store.is_node_visible_at_epoch(n, e) {
                return None;
            }
            match noeud.get_property("id") {
                Some(Value::Int64(i)) => Some(*i),
                autre => panic!("propriété id entière attendue, reçu {autre:?}"),
            }
        })
        .collect();
    v.sort_unstable();
    v
}

/// Avance l'époque par une écriture commitée sans rapport avec le test.
fn avance_epoque(db: &ObrainDB) -> EpochId {
    let avant = db.current_epoch();
    db.session()
        .execute("INSERT (:Horloge {tic: 1})")
        .expect("tic d'horloge");
    let apres = db.current_epoch();
    assert!(
        apres > avant,
        "une écriture autocommit doit avancer l'époque ({avant:?} → {apres:?})"
    );
    apres
}

/// Banc : trois nœuds `:T {id, v}` commités un par un, puis les `NodeId`
/// relevés par `id(n)`.
fn banc() -> (ObrainDB, Vec<NodeId>) {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for (id, v) in [(1, "a"), (2, "b"), (3, "a")] {
        s.execute(&format!("INSERT (:T {{id: {id}, v: '{v}'}})"))
            .expect("insertion");
    }
    let ids: Vec<NodeId> = s
        .execute("MATCH (n:T) RETURN id(n) ORDER BY n.id")
        .expect("id(n)")
        .rows
        .into_iter()
        .map(|l| match &l[0] {
            Value::Int64(i) => NodeId(*i as u64),
            autre => panic!("id(n) attendu, reçu {autre:?}"),
        })
        .collect();
    assert_eq!(ids.len(), 3, "trois nœuds :T");
    (db, ids)
}

/// Une assertion « même résultat » sur les deux oracles, pour un lecteur SANS
/// transaction à l'époque `e` (`None` = époque courante).
fn meme_resultat_sans_tx(
    db: &ObrainDB,
    candidats: &[NodeId],
    prop: &str,
    val_gql: &str,
    val: &Value,
    e: Option<EpochId>,
    contexte: &str,
) -> Vec<i64> {
    let lecteur = db.session();
    let filtre = entiers_a(
        &lecteur,
        &format!("MATCH (n:T {{{prop}: {val_gql}}}) RETURN n.id"),
        e,
    );
    let generique = entiers_a(
        &lecteur,
        &format!("MATCH (n) WHERE n.{prop} = {val_gql} RETURN n.id"),
        e,
    );
    let epoque = e.unwrap_or_else(|| db.current_epoch());
    let oracle = oracle_hydratation(db, candidats, prop, val, epoque);
    assert_eq!(
        filtre, oracle,
        "{contexte} : le filtre sans index doit rendre exactement ce que \
         l'hydratation (`get_node_at_epoch` + visibilité) rend à {epoque:?}"
    );
    assert_eq!(
        filtre, generique,
        "{contexte} : le filtre sans index doit rendre ce que le témoin \
         générique (sans label, prédicat à l'exécution) rend à {epoque:?}"
    );
    filtre
}

/// (a) Propriété MODIFIÉE par une transaction non commitée.
///
/// Lecteur tiers, époque courante et époque antérieure ; l'écrivain lui-même,
/// dans sa transaction ; puis après rollback. À chaque pas, les deux oracles.
#[test]
fn a_propriete_modifiee_non_commitee_meme_visibilite_que_l_hydratation() {
    let (db, ids) = banc();
    let e_avant = db.current_epoch();
    let e_apres = avance_epoque(&db);

    let mut ecrivain = db.session();
    ecrivain.begin_transaction().unwrap();
    ecrivain
        .execute("MATCH (n:T {id: 1}) SET n.v = 'z'")
        .expect("SET en transaction");

    // Le lecteur tiers, à l'époque courante : `v = 'a'` et `v = 'z'`.
    let a_courant = meme_resultat_sans_tx(
        &db,
        &ids,
        "v",
        "'a'",
        &Value::String("a".into()),
        None,
        "(a) lecteur tiers, époque courante, v = 'a'",
    );
    let z_courant = meme_resultat_sans_tx(
        &db,
        &ids,
        "v",
        "'z'",
        &Value::String("z".into()),
        None,
        "(a) lecteur tiers, époque courante, v = 'z'",
    );
    // Les deux ensembles se complètent : le nœud 1 est d'un côté ou de
    // l'autre, jamais des deux, jamais d'aucun.
    let mut union: Vec<i64> = a_courant.iter().chain(z_courant.iter()).copied().collect();
    union.sort_unstable();
    assert_eq!(
        union,
        vec![1, 3],
        "(a) le nœud 1 porte 'a' OU 'z', le 3 porte 'a'"
    );

    // À l'époque ANTÉRIEURE (avant le tic d'horloge) : même exigence. La
    // valeur rendue est celle que l'oracle rend — le substrat ne versionne
    // pas les propriétés, le test ne prétend pas le contraire.
    meme_resultat_sans_tx(
        &db,
        &ids,
        "v",
        "'a'",
        &Value::String("a".into()),
        Some(e_avant),
        "(a) lecteur tiers, époque antérieure, v = 'a'",
    );
    meme_resultat_sans_tx(
        &db,
        &ids,
        "v",
        "'z'",
        &Value::String("z".into()),
        Some(e_avant),
        "(a) lecteur tiers, époque antérieure, v = 'z'",
    );

    // L'écrivain voit sa propre écriture — et le filtre doit rendre ce que le
    // témoin générique rend dans la MÊME transaction.
    let z_ecrivain = entiers(&ecrivain, "MATCH (n:T {v: 'z'}) RETURN n.id");
    let z_ecrivain_generique = entiers(&ecrivain, "MATCH (n) WHERE n.v = 'z' RETURN n.id");
    assert_eq!(
        z_ecrivain, z_ecrivain_generique,
        "(a) écrivain en transaction : filtre et témoin générique d'accord"
    );
    assert_eq!(
        z_ecrivain,
        vec![1],
        "(a) l'écrivain voit sa propre écriture"
    );

    ecrivain.rollback().unwrap();
    let e_final = db.current_epoch();
    assert!(e_final >= e_apres);

    // Après rollback : 'a' est de retour sur 1 et 3, 'z' nulle part — par les
    // deux oracles.
    let a_final = meme_resultat_sans_tx(
        &db,
        &ids,
        "v",
        "'a'",
        &Value::String("a".into()),
        None,
        "(a) après rollback, v = 'a'",
    );
    assert_eq!(
        a_final,
        vec![1, 3],
        "(a) le rollback rend l'ancienne valeur"
    );
    let z_final = meme_resultat_sans_tx(
        &db,
        &ids,
        "v",
        "'z'",
        &Value::String("z".into()),
        None,
        "(a) après rollback, v = 'z'",
    );
    assert!(z_final.is_empty(), "(a) la valeur annulée a disparu");
}

/// (b) Nœud SUPPRIMÉ par une transaction non commitée : trouvé à l'ancienne
/// époque, pas à la nouvelle — et, à chaque époque, exactement ce que
/// l'hydratation rend.
#[test]
fn b_noeud_supprime_non_commite_ancienne_epoque_oui_nouvelle_non() {
    let (db, ids) = banc();
    let e_ancienne = db.current_epoch();
    let e_nouvelle = avance_epoque(&db);

    let mut ecrivain = db.session();
    ecrivain.begin_transaction().unwrap();
    ecrivain
        .execute("MATCH (n:T {id: 2}) DELETE n")
        .expect("DELETE en transaction");

    // Les candidats de l'oracle sont ceux du planificateur : `nodes_by_label`
    // — qui, comme le balayage complet, NE REND PAS les nœuds tombstonés
    // (« mêmes filtres vivacité (ALLOCATED + tombstone) »). C'est un fait
    // ANTÉRIEUR à C1, mesuré avant/après : par un balayage, le nœud supprimé
    // n'est retrouvé à AUCUNE époque, alors que l'accès direct
    // `get_node_at_epoch(id, ancienne)` le rend. Le contrat de ce test est
    // que C1 ne change rien à ce que le planificateur rend ; l'écart du
    // balayage est consigné ci-dessous, pas corrigé ici.
    let candidats = db.store().nodes_by_label("T");
    let nouvelle = meme_resultat_sans_tx(
        &db,
        &candidats,
        "id",
        "2",
        &Value::Int64(2),
        Some(e_nouvelle),
        "(b) nouvelle époque, id = 2",
    );
    let ancienne = meme_resultat_sans_tx(
        &db,
        &candidats,
        "id",
        "2",
        &Value::Int64(2),
        Some(e_ancienne),
        "(b) ancienne époque, id = 2",
    );
    assert!(
        nouvelle.is_empty(),
        "(b) le nœud supprimé (même non commité) n'est pas visible à l'époque \
         de sa suppression, obtenu {nouvelle:?}"
    );
    // L'accès DIRECT par époque voit le nœud supprimé à l'ancienne époque ;
    // les balayages (label ou complet) ne le proposent pas comme candidat.
    assert_eq!(
        oracle_hydratation(&db, &ids, "id", &Value::Int64(2), e_ancienne),
        vec![2],
        "(b) `get_node_at_epoch` rend le nœud supprimé à l'époque antérieure à sa mort"
    );
    assert!(
        !candidats.contains(&ids[1]),
        "(b) `nodes_by_label` ne propose pas le nœud tombstoné (fait antérieur à C1)"
    );
    assert!(
        ancienne.is_empty(),
        "(b) par balayage, le nœud supprimé n'est pas retrouvé non plus à \
         l'ancienne époque (écart consigné, identique avant/après C1), obtenu {ancienne:?}"
    );

    // Les autres nœuds ne bougent pas, aux deux époques.
    for e in [e_ancienne, e_nouvelle] {
        let a = meme_resultat_sans_tx(
            &db,
            &ids,
            "v",
            "'a'",
            &Value::String("a".into()),
            Some(e),
            "(b) v = 'a' aux deux époques",
        );
        assert_eq!(a, vec![1, 3]);
    }

    // L'écrivain, dans sa transaction : filtre et témoin générique d'accord.
    let chez_l_ecrivain = entiers(&ecrivain, "MATCH (n:T {id: 2}) RETURN n.id");
    let chez_l_ecrivain_generique = entiers(&ecrivain, "MATCH (n) WHERE n.id = 2 RETURN n.id");
    assert_eq!(
        chez_l_ecrivain, chez_l_ecrivain_generique,
        "(b) écrivain en transaction : filtre et témoin générique d'accord"
    );

    ecrivain.rollback().unwrap();
}

/// (b') Nœud CRÉÉ par une transaction non commitée : le second passage de
/// visibilité (la seule défense contre la lecture sale d'autrui sur ce chemin)
/// doit tenir — gql_13 le mesure déjà ; ici on le recoupe par l'oracle.
#[test]
fn b_prime_noeud_cree_non_commite_invisible_aux_tiers_visible_a_l_ecrivain() {
    let (db, mut ids) = banc();

    let mut ecrivain = db.session();
    ecrivain.begin_transaction().unwrap();
    ecrivain
        .execute("INSERT (:T {id: 4, v: 'a'})")
        .expect("INSERT en transaction");
    // Le candidat 4 est ajouté à la liste de l'oracle : `nodes_by_label` le
    // voit, c'est la visibilité qui doit l'écarter.
    ids.push(NodeId(ids.iter().map(|n| n.0).max().unwrap() + 1));

    let tiers = meme_resultat_sans_tx(
        &db,
        &ids,
        "v",
        "'a'",
        &Value::String("a".into()),
        None,
        "(b') lecteur tiers, nœud créé non commité",
    );
    assert_eq!(
        tiers,
        vec![1, 3],
        "(b') le tiers ne lit pas la création d'autrui"
    );

    let chez_l_ecrivain = entiers(&ecrivain, "MATCH (n:T {v: 'a'}) RETURN n.id");
    let chez_l_ecrivain_generique = entiers(&ecrivain, "MATCH (n) WHERE n.v = 'a' RETURN n.id");
    assert_eq!(chez_l_ecrivain, chez_l_ecrivain_generique);
    assert_eq!(
        chez_l_ecrivain,
        vec![1, 3, 4],
        "(b') l'écrivain voit sa création"
    );

    ecrivain.rollback().unwrap();
    let apres = meme_resultat_sans_tx(
        &db,
        &ids,
        "v",
        "'a'",
        &Value::String("a".into()),
        None,
        "(b') après rollback",
    );
    assert_eq!(apres, vec![1, 3]);
}

/// (c) LWW : trois écritures successives de la même clef — la dernière gagne,
/// sur le filtre comme sur l'oracle, et aucune des deux premières ne refait
/// surface par le chemin ciblé.
#[test]
fn c_derniere_ecriture_gagne_sur_le_filtre_comme_sur_l_hydratation() {
    let (db, ids) = banc();
    let s = db.session();
    for v in ["v1", "v2", "v3"] {
        s.execute(&format!("MATCH (n:T {{id: 2}}) SET n.v = '{v}'"))
            .expect("SET");
    }

    let v3 = meme_resultat_sans_tx(
        &db,
        &ids,
        "v",
        "'v3'",
        &Value::String("v3".into()),
        None,
        "(c) v = 'v3'",
    );
    assert_eq!(v3, vec![2], "(c) la dernière écriture gagne");
    for ancienne in ["'v1'", "'v2'", "'b'"] {
        let r = meme_resultat_sans_tx(
            &db,
            &ids,
            "v",
            ancienne,
            &Value::String(ancienne.trim_matches('\'').into()),
            None,
            "(c) valeur écrasée",
        );
        assert!(r.is_empty(), "(c) {ancienne} a été écrasée, obtenu {r:?}");
    }

    // AND de deux égalités : la seconde propriété n'est lue que pour les
    // survivants de la première — le résultat doit être l'intersection.
    let lecteur = db.session();
    let et = entiers(&lecteur, "MATCH (n:T {v: 'a', id: 3}) RETURN n.id");
    let et_generique = entiers(
        &lecteur,
        "MATCH (n) WHERE n.v = 'a' AND n.id = 3 RETURN n.id",
    );
    assert_eq!(et, et_generique);
    assert_eq!(et, vec![3]);
    let vide = entiers(&lecteur, "MATCH (n:T {v: 'a', id: 2}) RETURN n.id");
    assert!(vide.is_empty(), "(c) AND sans intersection : vide");
}
