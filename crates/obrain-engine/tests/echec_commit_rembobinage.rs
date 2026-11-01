//! Un commit REFUSÉ (conflit de validation) ne laisse rien derrière lui —
//! ni visible, ni SUR LE DISQUE.
//!
//! (2026-11-01, audit v0.0.1) Dans la branche d'erreur de `commit_inner`,
//! `tx_baselines` était vidé AVANT l'appel à `rembobine_aux_empreintes`, qui
//! lisait donc une liste vide et ne rembobinait rien ; `discard_uncommitted_
//! versions` n'était pas appelé non plus. Les nœuds et arêtes créés par la
//! transaction refusée restaient ALLOUÉS dans les zones du substrat, cachés
//! seulement par leur marqueur « en attente » en mémoire.
//!
//! MESURÉ sur main e11bdd2a : dans le même processus, `MATCH` ne les voit
//! pas (le marqueur en mémoire les cache) — les quatre premiers tests
//! passaient déjà. Mais après `close()` + `open()`, le marqueur a disparu et
//! les entités REVIENNENT : c'est le cinquième test, rouge sur main, vert
//! avec le rembobinage réel (tombstones physiques, journalisées).
//!
//! Déterministe : deux sessions, un thread. Le conflit est provoqué au
//! COMMIT (et non à l'écriture) : S2 commite une modification de X APRÈS le
//! début de S1 ; S1 modifie X aussi, son `record_write` passe (S2 n'est plus
//! active), et c'est la validation du commit qui refuse.

use obrain_engine::ObrainDB;

fn visibles(db: &ObrainDB, requete: &str) -> usize {
    db.session().execute(requete).unwrap().row_count()
}

fn provoque_un_commit_refuse(db: &ObrainDB) {
    db.session().execute("INSERT (:X {p: 0})").unwrap();

    let mut s1 = db.session();
    s1.begin_transaction().unwrap();
    // S1 doit avoir une empreinte posée AVANT la création : la première
    // écriture de la transaction fait l'affaire.
    s1.execute("INSERT (:Refuse {who: 'S1-a'})").unwrap();

    let mut s2 = db.session();
    s2.begin_transaction().unwrap();
    s2.execute("MATCH (x:X) SET x.p = 2").unwrap();
    s2.commit().unwrap();

    s1.execute("INSERT (:Refuse {who: 'S1-b'})-[:REFUSEE]->(:Refuse {who: 'S1-c'})")
        .unwrap();
    s1.execute("MATCH (x:X) SET x.p = 1").unwrap();

    let refus = s1.commit();
    assert!(
        refus.is_err(),
        "le scénario devait produire un conflit de validation, il a commité : {refus:?}"
    );
}

#[test]
fn un_commit_refuse_ne_laisse_aucune_arete_visible() {
    let db = ObrainDB::new_in_memory();
    provoque_un_commit_refuse(&db);
    assert_eq!(
        visibles(&db, "MATCH ()-[r:REFUSEE]->() RETURN r"),
        0,
        "l'arête d'une transaction refusée est restée visible"
    );
}

#[test]
fn un_commit_refuse_ne_laisse_aucun_noeud_visible() {
    let db = ObrainDB::new_in_memory();
    provoque_un_commit_refuse(&db);
    assert_eq!(
        visibles(&db, "MATCH (n:Refuse) RETURN n"),
        0,
        "des nœuds d'une transaction refusée sont restés visibles"
    );
}

#[test]
fn un_commit_refuse_ne_touche_pas_a_la_valeur_commitee_par_l_autre() {
    let db = ObrainDB::new_in_memory();
    provoque_un_commit_refuse(&db);
    assert_eq!(
        visibles(&db, "MATCH (x:X {p: 2}) RETURN x"),
        1,
        "la valeur commitée par S2 a été perdue ou écrasée par le refus de S1"
    );
}

#[test]
fn apres_un_commit_refuse_la_session_peut_recommencer() {
    let db = ObrainDB::new_in_memory();
    provoque_un_commit_refuse(&db);
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:Apres {ok: true})").unwrap();
    s.commit()
        .expect("une nouvelle transaction après un refus doit commiter");
    assert_eq!(visibles(&db, "MATCH (n:Apres) RETURN n"), 1);
}

/// LE test discriminant : ce qu'un commit refusé laisse sur le disque.
#[test]
fn apres_reouverture_un_commit_refuse_n_a_laisse_aucune_entite() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("base");
    {
        let db = ObrainDB::open(&chemin).unwrap();
        provoque_un_commit_refuse(&db);
        // Dans le processus : invisibles (marqueur en mémoire).
        assert_eq!(visibles(&db, "MATCH (n:Refuse) RETURN n"), 0);
        db.close().unwrap();
    }
    let db = ObrainDB::open(&chemin).unwrap();
    assert_eq!(
        visibles(&db, "MATCH (n:Refuse) RETURN n"),
        0,
        "les nœuds d'une transaction REFUSÉE sont revenus à la réouverture"
    );
    assert_eq!(
        visibles(&db, "MATCH ()-[r:REFUSEE]->() RETURN r"),
        0,
        "les arêtes d'une transaction REFUSÉE sont revenues à la réouverture"
    );
    assert_eq!(visibles(&db, "MATCH (x:X {p: 2}) RETURN x"), 1);
}
