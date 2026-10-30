//! UNE ECRITURE PLACEE SOUS UN SCAN RESTE UNE ECRITURE.
//!
//! ── LA CLASSE DE DEFAUT (revue de `99fb966f`, 2026-08-24) ────────────
//!
//! `has_mutations()` re-declarait la forme de l'arbre a la main et mentait
//! sur SEPT operateurs — `NodeScan`, `EdgeScan`, `TripleScan`, `VectorScan`
//! (enfant optionnel), `Expand`, `ShortestPath`, `VectorJoin` (enfant
//! REQUIS) — qu'elle traitait en feuilles alors que `children()`, dans le
//! meme `impl`, leur donne un enfant. Une ecriture placee sous l'un d'eux
//! etait invisible a la frontiere, tout en etant PRESENTE dans le plan.
//!
//! Il suffit d'un second `MATCH` apres l'ecriture pour que le plan la range
//! sous un scan. Les deux requetes ci-dessous, mesurees par la revue sur le
//! commit precedent :
//!
//!   · `MATCH (b:Big) CREATE (c:Copie ...) MATCH (z:S) RETURN count(z)`
//!     rendait `false` — annulee en vol, elle persistait des milliers de
//!     noeuds sans transaction ni retour arriere ;
//!   · `MATCH (b:Big) DETACH DELETE b MATCH (z:S) RETURN count(z)`
//!     rendait `false` aussi — et DETRUISAIT des milliers de noeuds sur une
//!     requete que le moteur declarait annulee.
//!
//! La classe est fermee par derivation : `has_mutations` demande desormais
//! ses enfants a `children()`. Le VERROU de cette propriete est le module
//! `verrou_structurel` de `plan.rs` ; ce fichier-ci en verifie l'effet
//! observable, plus l'anti-vacuite qui empeche les tests de passer pour la
//! mauvaise raison.

#![cfg(feature = "gql")]

use std::time::Duration;

use obrain_common::types::Value;
use obrain_common::utils::cancel::{self, CancelToken};
use obrain_engine::ObrainDB;

const NOEUDS: i64 = 30_000;
const ARETES: i64 = 20_000;

/// L'ecriture est SOUS un scan : le second `MATCH` la range sous lui.
const ECRITURE_SOUS_SCAN: &str =
    "MATCH (b:Big) CREATE (c:Copie {v: b.v}) MATCH (z:Sentinelle) RETURN count(z)";
/// Temoin d'anti-vacuite : la MEME ecriture, sans le second `MATCH`.
const ECRITURE_NUE: &str = "MATCH (b:Big) CREATE (c:Copie {v: b.v})";

const SUPPRESSION_SOUS_SCAN: &str =
    "MATCH (b:Big) DETACH DELETE b MATCH (z:Sentinelle) RETURN count(z)";

// ─────────────────────────────────────────────────────────────────────
//  I. LA CLASSIFICATION ELLE-MEME, SANS PASSER PAR LE MOTEUR
// ─────────────────────────────────────────────────────────────────────

fn mute(requete: &str) -> bool {
    let plan = obrain_engine::query::translators::gql::translate(requete)
        .unwrap_or_else(|e| panic!("traduction de `{requete}` : {e}"));
    plan.root.has_mutations()
}

#[test]
fn une_ecriture_sous_un_scan_est_classee_ecriture() {
    assert!(
        mute(ECRITURE_SOUS_SCAN),
        "has_mutations() rend false sur `{ECRITURE_SOUS_SCAN}` : le CreateNode \
         est pourtant DANS le plan, sous un scan. C'est la classe de defaut \
         qui a persiste des milliers de noeuds sur une requete annulee."
    );
}

#[test]
fn temoin_anti_vacuite_l_ecriture_nue_est_deja_classee_ecriture() {
    // Sans ce temoin, le test precedent passerait meme si `has_mutations()`
    // rendait `true` pour absolument tout.
    assert!(mute(ECRITURE_NUE), "l'ecriture nue doit etre classee ecriture");
    assert!(
        !mute("MATCH (b:Big) RETURN b.v"),
        "ANTI-VACUITE : une LECTURE pure doit rester classee lecture, sinon \
         `has_mutations()` rendrait `true` partout et ne prouverait rien"
    );
}

#[test]
fn une_suppression_sous_un_scan_est_classee_ecriture() {
    assert!(
        mute(SUPPRESSION_SOUS_SCAN),
        "has_mutations() rend false sur `{SUPPRESSION_SOUS_SCAN}` : un \
         DETACH DELETE annule en vol DETRUIT des donnees definitivement."
    );
}

// ─────────────────────────────────────────────────────────────────────
//  II. L'EFFET OBSERVABLE, SUR UNE VRAIE BASE
// ─────────────────────────────────────────────────────────────────────

fn base_ecriture() -> (tempfile::TempDir, ObrainDB) {
    use obrain_core::graph::traits::GraphStoreMut;
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("b");
    {
        let store = obrain_substrate::SubstrateStore::create(&chemin).expect("creation");
        for i in 0..NOEUDS {
            let n = store.create_node(&["Big"]);
            store.set_node_property(n, "v", Value::Int64(i));
        }
        // La sentinelle donne au second MATCH quelque chose a scanner.
        store.create_node(&["Sentinelle"]);
    }
    let db = ObrainDB::open_substrate(&chemin).expect("ouverture");
    (td, db)
}

fn base_suppression() -> (tempfile::TempDir, ObrainDB) {
    use obrain_core::graph::traits::GraphStoreMut;
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("b");
    {
        let store = obrain_substrate::SubstrateStore::create(&chemin).expect("creation");
        let mut precedent = None;
        for i in 0..ARETES {
            let n = store.create_node(&["Big"]);
            store.set_node_property(n, "v", Value::Int64(i));
            if let Some(p) = precedent {
                let _ = store.create_edge(p, n, "R");
            }
            precedent = Some(n);
        }
        store.create_node(&["Sentinelle"]);
    }
    let db = ObrainDB::open_substrate(&chemin).expect("ouverture");
    (td, db)
}

fn compte(db: &ObrainDB, requete: &str) -> i64 {
    let q = db.session().execute(requete).expect("relecture");
    match q.rows[0][0] {
        Value::Int64(n) => n,
        ref autre => panic!("compte inattendu : {autre:?}"),
    }
}

fn armer_pendant(db: &ObrainDB, requete: &str) -> Result<(), String> {
    let jeton = CancelToken::new();
    let garde = jeton.clone();
    let chien = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        garde.cancel();
    });
    let r = cancel::scope(&jeton, || db.session().execute(requete));
    chien.join().unwrap();
    r.map(|_| ()).map_err(|e| format!("{e:?}"))
}

#[test]
fn une_ecriture_sous_un_scan_annulee_en_vol_ne_laisse_rien_a_moitie() {
    let (_td, db) = base_ecriture();
    let r = armer_pendant(&db, ECRITURE_SOUS_SCAN);
    let n = compte(&db, "MATCH (c:Copie) RETURN count(c)");
    assert!(
        n == 0 || n == NOEUDS,
        "ECRITURE FAUSSE : {n} noeuds persistes sur {NOEUDS}. Une ecriture \
         placee sous un scan doit soit aboutir, soit ne RIEN laisser — \
         jamais un etat intermediaire, puisque personne ne le defera. \
         Resultat : {r:?}"
    );
    assert_eq!(
        n, NOEUDS,
        "et la frontiere faisant son office, l'ecriture va a son terme"
    );
}

#[test]
fn une_suppression_sous_un_scan_annulee_en_vol_ne_detruit_pas_a_moitie() {
    let (_td, db) = base_suppression();
    let avant = compte(&db, "MATCH (b:Big) RETURN count(b)");
    assert_eq!(avant, ARETES, "fixture");

    let r = armer_pendant(&db, SUPPRESSION_SOUS_SCAN);
    let reste = compte(&db, "MATCH (b:Big) RETURN count(b)");
    assert!(
        reste == avant || reste == 0,
        "SUPPRESSION FAUSSE : {reste} noeuds restants sur {avant}, donc \
         {} DETRUITS sur une requete que le moteur declare annulee. La \
         destruction de donnees ne se rattrape pas. Resultat : {r:?}",
        avant - reste
    );
    assert_eq!(reste, 0, "une suppression n'est pas annulable : elle aboutit");
}

/// ANTI-VACUITE DE LA SECTION II : le jeton employe est bien vivant, et il
/// annule bien une LECTURE de meme forme sur la meme base.
#[test]
fn antivacuite_le_meme_dispositif_annule_bien_une_lecture() {
    let (_td, db) = base_ecriture();
    let jeton = CancelToken::new();
    jeton.cancel();
    let r = cancel::scope(&jeton, || {
        db.session()
            .execute("MATCH (b:Big) RETURN b.v ORDER BY b.v")
    });
    assert!(
        r.as_ref().err().is_some_and(|e| e.is_cancelled()),
        "si ce test echoue, les deux precedents passent pour la mauvaise \
         raison : le jeton serait inerte. Obtenu : {r:?}"
    );
}
