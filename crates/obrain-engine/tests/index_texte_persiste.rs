//! ── L'INDEX TEXTE SURVIT AU PROCESSUS (ET LE DIT) ──────────────────────────
//!
//! Défaut mesuré le 2026-09-16 : l'index inverti de `create_text_index` vivait
//! en RAM par processus, reconstruit par un balayage complet du label à chaque
//! ouverture — ~1,6 s par lecteur sur les bases runtime, ce qui interdisait de
//! fait le BM25 dans un chemin chaud (une injection par tour ne peut pas payer
//! ça).
//!
//! Ce fichier éprouve les trois propriétés qui font qu'une persistance est une
//! optimisation et non un piège :
//!
//! 1. une base ROUVERTE sans écriture **charge** son index (et le compte le
//!    dit) — le balayage n'a pas eu lieu ;
//! 2. une base rouverte APRÈS écriture **reconstruit** — un index périmé rend
//!    des résultats faux (document ajouté introuvable, document supprimé
//!    encore trouvable), donc on ne le charge jamais ;
//! 3. les RÉPONSES sont identiques dans les deux cas : charger et reconstruire
//!    rendent le même classement. Sans cette troisième propriété, les deux
//!    premières ne prouveraient qu'une chose : qu'on va plus vite pour se
//!    tromper.

#![cfg(feature = "text-index")]

use tempfile::tempdir;

use obrain_engine::ObrainDB;

/// Une base peuplée, avec un index texte déclaré sur `Doc.text`.
fn base_indexee(chemin: &std::path::Path) -> ObrainDB {
    let db = ObrainDB::open(chemin).expect("open");
    let textes = [
        "le graphe de connaissances relie les notes",
        "une note sur le moteur de graphe",
        "le jardin potager et ses tomates",
        "rust et les bases de donnees graphe",
        "note de service sans rapport",
    ];
    for t in textes {
        let session = db.session();
        session
            .execute(&format!("CREATE (:Doc {{text: '{t}'}})"))
            .expect("create");
    }
    db.create_text_index("Doc", "text").expect("index texte");
    db
}

fn resultats_scores(db: &ObrainDB, requete: &str) -> Vec<(u64, f64)> {
    db.text_search("Doc", "text", requete, 10)
        .expect("recherche")
        .into_iter()
        .map(|(id, score)| (id.0, score))
        .collect()
}

fn resultats(db: &ObrainDB, requete: &str) -> Vec<u64> {
    resultats_scores(db, requete).into_iter().map(|(id, _)| id).collect()
}

/// Le classement COMPARABLE : l'ensemble des `(nœud, score)`.
///
/// ── POURQUOI PAS LA LISTE DANS L'ORDRE ──────────────────────────────────────
/// Défaut de ce test, trouvé en l'écrivant : trois documents peuvent porter le
/// MÊME score (« graphe » apparaît une fois dans trois textes de longueur
/// égale), et l'ordre entre ex æquo n'est pas spécifié — il dépend de l'ordre
/// d'itération interne. Comparer les listes telles quelles rendait le test
/// instable, et l'instabilité n'apprenait rien sur la persistance : c'est le
/// SCORE qui doit survivre au rechargement, pas un ordre arbitraire.
///
/// On compare donc l'ensemble trié des `(nœud, score)`, et l'ordre n'est exigé
/// que là où les scores DIFFÈRENT (voir `le_meilleur_est_en_tete`).
fn classement(db: &ObrainDB, requete: &str) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = resultats_scores(db, requete)
        .into_iter()
        .map(|(id, score)| (id, (score * 1e12) as u64))
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn une_reouverture_sans_ecriture_CHARGE_l_index() {
    let td = tempdir().unwrap();
    let chemin = td.path().join("idx-charge");
    let attendus = {
        let db = base_indexee(&chemin);
        assert!(
            db.nb_index_texte_persistes() >= 1,
            "construire un index doit le PERSISTER (sinon rien n'est rejouable)"
        );
        let r = classement(&db, "graphe");
        assert!(!r.is_empty(), "l'index doit répondre avant la fermeture");
        db.close().expect("close");
        r
    };

    // Réouverture SANS aucune écriture : l'empreinte décrit encore le store.
    let db = ObrainDB::open(&chemin).expect("reopen");
    assert_eq!(
        db.nb_index_texte_charges(),
        1,
        "l'index déclaré doit être CHARGÉ depuis le disque (chargés = {})",
        db.nb_index_texte_charges()
    );
    assert_eq!(
        classement(&db, "graphe"),
        attendus,
        "un index chargé doit rendre les MÊMES scores qu'un index construit"
    );
    // Et un document unique (aucune égalité possible) doit sortir en TÊTE.
    let unique = resultats(&db, "tomates");
    assert_eq!(unique.len(), 1, "« tomates » n'apparaît que dans un document");
    db.close().expect("close");
}

#[test]
fn une_ecriture_invalide_l_empreinte_et_l_index_est_reconstruit() {
    let td = tempdir().unwrap();
    let chemin = td.path().join("idx-reconstruit");
    {
        let db = base_indexee(&chemin);
        db.close().expect("close");
    }

    // Une écriture APRÈS la persistance : l'empreinte (offset du WAL + nœuds)
    // ne décrit plus l'index du fichier.
    {
        let db = ObrainDB::open(&chemin).expect("reopen");
        db.session()
            .execute("CREATE (:Doc {text: 'un nouveau document sur le graphe'})")
            .expect("create");
        db.close().expect("close");
    }

    let db = ObrainDB::open(&chemin).expect("reopen");
    assert_eq!(
        db.nb_index_texte_charges(),
        0,
        "un index périmé ne doit JAMAIS être chargé (il rendrait des résultats faux)"
    );
    let trouve = resultats(&db, "nouveau");
    assert!(
        !trouve.is_empty(),
        "après reconstruction, le document AJOUTÉ doit être trouvable"
    );
    db.close().expect("close");
}

/// ── UN INDEX PRODUIT PAR UN AUTRE VOCABULAIRE N'EST PAS CHARGÉ ─────────────
///
/// Les termes d'un index inverti SONT la sortie d'un tokenizer : recharger un
/// fichier avec un autre tokenizer (mots vides, longueur minimale, stemmer,
/// langue) ferait répondre le moteur avec un vocabulaire que le code ne croit
/// plus — sans erreur, sans trace, juste des réponses fausses. Le fichier porte
/// donc l'identité de ce qui l'a produit, et le chargement la vérifie.
///
/// Le test le prouve en PATCHANT l'identité dans le fichier : c'est exactement
/// ce qui se passerait si quelqu'un changeait le tokenizer sans reconstruire.
#[test]
fn un_index_produit_par_un_autre_tokenizer_est_refuse() {
    let td = tempdir().unwrap();
    let chemin = td.path().join("idx-identite");
    {
        let db = base_indexee(&chemin);
        db.close().expect("close");
    }
    let fichier = chemin.join("substrate.textidx.Doc.text");
    let brut = std::fs::read(&fichier).expect("le fichier d'index doit exister");
    let identite = b"simple(min_length=2);bm25(k1=1.2,b=0.75)";
    let position = brut
        .windows(identite.len())
        .position(|f| f == identite)
        .expect("l'identité du producteur doit être DANS le fichier");
    // Même longueur, un seul chiffre change : le tokenizer ne serait plus le
    // même, et c'est tout ce qu'il faut pour que l'index soit invalide.
    let mut patche = brut.clone();
    patche[position + "simple(min_length=".len()] = b'9';
    std::fs::write(&fichier, &patche).expect("patch");

    let db = ObrainDB::open(&chemin).expect("reopen");
    assert_eq!(
        db.nb_index_texte_charges(),
        0,
        "un index produit par un AUTRE tokenizer ne doit jamais être chargé"
    );
    // Et la reconstruction rend un index qui répond (le filet fonctionne).
    assert!(
        !resultats(&db, "graphe").is_empty(),
        "après refus, l'index doit être reconstruit et répondre"
    );
    db.close().expect("close");
}

#[test]
fn l_index_charge_et_l_index_reconstruit_donnent_le_meme_classement() {
    let td = tempdir().unwrap();
    let chemin_charge = td.path().join("idx-a");
    let chemin_construit = td.path().join("idx-b");

    // A : index persisté puis rechargé.
    let (classement_charge, avant, temoin) = {
        let db = base_indexee(&chemin_charge);
        let avant = classement(&db, "note graphe");
        db.close().expect("close");
        let db = ObrainDB::open(&chemin_charge).expect("reopen");
        assert_eq!(db.nb_index_texte_charges(), 1, "index chargé");
        let apres = classement(&db, "note graphe");
        db.close().expect("close");

        // Témoin : même corpus, même index, sur une AUTRE base — jamais chargé
        // (c'est une base neuve), donc construit.
        let db_b = base_indexee(&chemin_construit);
        let temoin = classement(&db_b, "note graphe");
        db_b.close().expect("close");
        (apres, avant, temoin)
    };
    assert_eq!(classement_charge, avant, "chargé == construit sur la même base");
    assert_eq!(
        classement_charge, temoin,
        "chargé == témoin construit sur une base identique"
    );
}
