//! Ce que l'agent a rapporte le 2026-08-14 : `MATCH (p1), (p2) CREATE
//! (p1)-[:SHARES_DOMAIN]->(p2)` rend `{}` — ni erreur, ni arete. Il a verifie
//! trois fois avec `MATCH ()-[r]->() RETURN type(r), count(*)` et lu zero.
//!
//! Ces tests separent les trois formes qui devraient etre equivalentes, pour
//! savoir laquelle ment :
//!   - deux clauses MATCH successives (forme deja couverte ailleurs),
//!   - une clause MATCH a deux motifs separes par une virgule (SA forme),
//!   - le produit cartesien seul, sans CREATE, pour savoir si le MATCH lui-meme
//!     produit bien les paires.
//!
//! Un echec ici n'est pas un defaut de confort : une ecriture qui rend un
//! succes vide est une perte de donnee silencieuse. La base doit refuser ou
//! ecrire, jamais faire semblant.

use obrain_engine::ObrainDB;

fn base() -> (ObrainDB, tempfile::TempDir) {
    let dossier = tempfile::tempdir().expect("dossier temporaire");
    let db = ObrainDB::open(dossier.path()).expect("ouverture");
    (db, dossier)
}

fn compte_aretes(db: &ObrainDB) -> usize {
    let r = db
        .execute("MATCH ()-[r]->() RETURN r")
        .expect("comptage des aretes");
    r.rows.len()
}

fn deux_noeuds(db: &ObrainDB) {
    db.execute("CREATE (:P {id: '1'})").expect("noeud 1");
    db.execute("CREATE (:P {id: '2'})").expect("noeud 2");
}

/// Temoin : la forme a deux clauses MATCH. Si celle-ci echoue aussi, le defaut
/// n'est pas dans le parsing de la virgule.
#[test]
fn deux_clauses_match_creent_l_arete() {
    let (db, _dossier) = base();
    deux_noeuds(&db);

    let r = db
        .execute("MATCH (a:P {id: '1'}) MATCH (b:P {id: '2'}) CREATE (a)-[:LIE]->(b)")
        .expect("la creation ne doit pas echouer");
    let _ = r;

    assert_eq!(
        compte_aretes(&db),
        1,
        "deux clauses MATCH successives doivent produire exactement une arete"
    );
}

/// LA forme de l'agent. Deux motifs, une virgule, une seule clause MATCH.
#[test]
fn match_a_virgule_cree_l_arete() {
    let (db, _dossier) = base();
    deux_noeuds(&db);

    let r = db
        .execute("MATCH (a:P {id: '1'}), (b:P {id: '2'}) CREATE (a)-[:LIE]->(b)")
        .expect("la creation ne doit pas echouer");
    let _ = r;

    assert_eq!(
        compte_aretes(&db),
        1,
        "un MATCH a deux motifs separes par une virgule doit produire une arete"
    );
}

/// Le MATCH a virgule produit-il seulement les paires ? Si ce compte est bon
/// mais que l'arete manque, le defaut est dans le CREATE ; s'il est faux, le
/// defaut est en amont, dans le produit cartesien.
#[test]
fn match_a_virgule_produit_bien_la_paire() {
    let (db, _dossier) = base();
    deux_noeuds(&db);

    let r = db
        .execute("MATCH (a:P {id: '1'}), (b:P {id: '2'}) RETURN a, b")
        .expect("le MATCH seul doit reussir");

    assert_eq!(
        r.rows.len(),
        1,
        "deux motifs joignant un noeud chacun doivent rendre une paire"
    );
}

/// La forme la plus large : sans filtre, deux motifs libres. C'est ce que
/// l'agent ecrit quand il relie « tout a tout » dans un petit ensemble.
#[test]
fn match_a_virgule_sans_filtre_cree_les_aretes() {
    let (db, _dossier) = base();
    deux_noeuds(&db);

    let r = db
        .execute("MATCH (a:P), (b:P) CREATE (a)-[:LIE]->(b)")
        .expect("la creation ne doit pas echouer");
    let _ = r;

    assert_eq!(
        compte_aretes(&db),
        4,
        "2 noeuds x 2 noeuds = 4 paires, donc 4 aretes (boucles comprises)"
    );
}

/// Le point le plus important, independamment de la cause : si la base ne peut
/// PAS creer l'arete, elle doit le DIRE. Un resultat vide qui se lit comme un
/// succes est le pire des trois etats possibles.
#[test]
fn une_creation_qui_n_ecrit_rien_ne_doit_pas_se_lire_comme_un_succes() {
    let (db, _dossier) = base();
    deux_noeuds(&db);

    let issue = db.execute("MATCH (a:P {id: '1'}), (b:P {id: '2'}) CREATE (a)-[:LIE]->(b)");

    match issue {
        Err(_) => {
            // Refus explicite : acceptable. L'appelant sait.
        }
        Ok(_) => {
            assert_eq!(
                compte_aretes(&db),
                1,
                "la base a rendu un succes : l'arete DOIT exister"
            );
        }
    }
}
