//! UN `CALL` DE PROCEDURE QUI ECRIT N EST PAS ANNULABLE.
//!
//! ── LE DEFAUT QUE CE FICHIER FIGE (revue du delta, 2026-08-24) ───────
//!
//! `LogicalOperator::has_mutations()` classait `CallProcedure` parmi les
//! feuilles LECTURE SEULE. Or `UserProcedureOperator` retraduit, replanifie
//! et execute un corps GQL arbitraire, ecritures comprises — et le corps
//! n est pas dans le plan, il est en TEXTE dans le catalogue.
//!
//! Les deux consequences allaient du mauvais cote :
//!
//!   · la frontiere mettait le jeton EN VIGUEUR sur un plan qui ecrit ;
//!   · `needs_auto_commit(false)` n ouvrait AUCUNE transaction implicite,
//!     donc il n y avait rien a defaire.
//!
//! Mesure de la revue : 30 000 noeuds, jeton arme a 40 ms, resultat
//! `Err(... query cancelled)` et 4 096 noeuds PERSISTES. Ni 0 ni 30 000 —
//! une ecriture FAUSSE, la categorie que l inversion de polarite devait
//! rendre impossible. Elle ne la rendait impossible que pour les plans
//! dont `has_mutations()` dit la verite.
//!
//!   cargo test -p obrain-engine --features algos --test procedure_qui_ecrit

#![cfg(all(feature = "algos", feature = "gql"))]

use std::time::Duration;

use obrain_common::types::Value;
use obrain_common::utils::cancel::{self, CancelToken};
use obrain_engine::ObrainDB;

/// Assez de noeuds pour que la copie dure largement plus que le delai
/// d armement : sans cela, « annule en vol » ne veut rien dire.
const NOEUDS: i64 = 30_000;

const CORPS: &str = "CREATE PROCEDURE copie() RETURNS (n INT) AS \
     { MATCH (b:Big) CREATE (d:Copie {v: b.v}) RETURN count(d) AS n }";

fn base() -> (tempfile::TempDir, ObrainDB) {
    use obrain_core::graph::traits::GraphStoreMut;
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("b");
    {
        let store = obrain_substrate::SubstrateStore::create(&chemin).expect("creation");
        for i in 0..NOEUDS {
            let n = store.create_node(&["Big"]);
            store.set_node_property(n, "v", Value::Int64(i));
        }
    }
    let db = ObrainDB::open_substrate(&chemin).expect("ouverture");
    db.session().execute(CORPS).expect("declaration procedure");
    (td, db)
}

fn copies(db: &ObrainDB) -> i64 {
    let q = db
        .session()
        .execute("MATCH (c:Copie) RETURN count(c)")
        .expect("relecture");
    match q.rows[0][0] {
        Value::Int64(n) => n,
        ref autre => panic!("compte inattendu : {autre:?}"),
    }
}

/// ANTI-VACUITE : sans jeton, la procedure ecrit bien les 30 000.
/// Si ce test echouait, les deux suivants ne prouveraient rien.
#[test]
fn antivacuite_la_procedure_ecrit_bien_sans_jeton() {
    let (_td, db) = base();
    let r = db.session().execute("CALL copie()");
    assert!(r.is_ok(), "CALL doit reussir sans jeton : {r:?}");
    assert_eq!(
        copies(&db),
        NOEUDS,
        "la procedure doit copier tous les noeuds"
    );
}

/// Jeton arme AVANT le depart : la meme forme que le temoin d ecriture
/// litterale. Une procedure qui ecrit n est pas annulable.
#[test]
fn un_call_qui_ecrit_ignore_un_jeton_deja_arme() {
    let (_td, db) = base();
    let jeton = CancelToken::new();
    jeton.cancel();
    let r = cancel::scope(&jeton, || db.session().execute("CALL copie()"));
    assert!(
        !r.as_ref().err().is_some_and(|e| e.is_cancelled()),
        "un CALL qui ECRIT ne doit pas etre annulable : {r:?}"
    );
    assert_eq!(
        copies(&db),
        NOEUDS,
        "l ecriture de la procedure doit avoir eu lieu en entier"
    );
}

/// LE TEST DU BLOQUANT. Jeton arme EN VOL, au milieu de la copie.
///
/// Le seul resultat acceptable est TOUT ou RIEN. La mesure de la revue —
/// 4 096 sur 30 000 — est ce que ce test refuse.
#[test]
fn un_call_annule_en_vol_ne_laisse_jamais_d_ecriture_partielle() {
    let (_td, db) = base();

    let jeton = CancelToken::new();
    let garde = jeton.clone();
    let chien = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        garde.cancel();
    });
    let r = cancel::scope(&jeton, || db.session().execute("CALL copie()"));
    chien.join().unwrap();

    let n = copies(&db);
    assert!(
        n == 0 || n == NOEUDS,
        "ECRITURE FAUSSE : {n} noeuds persistes sur {NOEUDS}. Un CALL annule \
         en vol doit soit aboutir, soit ne RIEN laisser — jamais un etat \
         intermediaire, puisque personne ne le defera. Resultat de la \
         requete : {r:?}"
    );
    // Et, la frontiere faisant son office, c est TOUT.
    assert_eq!(
        n, NOEUDS,
        "une ecriture n est pas annulable : elle doit aller a son terme"
    );
}

// ─────────────────────────────────────────────────────────────────────
//  II. LA CONTREPARTIE : UN `CALL` QUI LIT RESTE ANNULABLE
// ─────────────────────────────────────────────────────────────────────
//
// Une version anterieure de ce correctif rendait `CallProcedure` mutation
// EN BLOC. C'etait une regression, pas une precaution : `CallProcedureOp`
// couvre six familles dont cinq LISENT — introspection, projection,
// cognitif, et les algorithmes integres `pagerank` / `sssp`. La branche est
// nee de « 700-800 % de CPU en permanence » ; retirer l'annulation
// justement au calcul long etait a contresens.
//
// Le partage est desormais fait par le catalogue, avec le test EXACT du
// planificateur : `catalog.get_procedure(dernier_segment).is_some()`.
// Les deux tests ci-dessous sont la meme construction `CALL`, avec des
// issues opposees — c'est ce couple qui prouve que le partage a lieu.

#[test]
fn un_call_d_algorithme_integre_reste_annulable() {
    let (_td, db) = base();
    let jeton = CancelToken::new();
    jeton.cancel();
    let r = cancel::scope(&jeton, || db.session().execute("CALL obrain.pagerank()"));
    assert!(
        r.as_ref().err().is_some_and(|e| e.is_cancelled()),
        "REGRESSION : `CALL obrain.pagerank` n est plus annulable. Aucune \
         procedure utilisateur ne s appelle `pagerank` dans cette base ; le \
         catalogue doit donc le classer en LECTURE. Obtenu : {r:?}"
    );
}

#[test]
fn un_call_d_introspection_reste_annulable() {
    let (_td, db) = base();
    let jeton = CancelToken::new();
    jeton.cancel();
    let r = cancel::scope(&jeton, || db.session().execute("CALL obrain.procedures()"));
    assert!(
        r.as_ref().err().is_some_and(|e| e.is_cancelled()),
        "`CALL obrain.procedures()` est une lecture pure : elle doit rester \
         annulable. Obtenu : {r:?}"
    );
}

/// ANTI-VACUITE DU COUPLE : sur la MEME base, au MEME instant, le `CALL`
/// d une procedure UTILISATEUR n est PAS annulable. Sans ce contraste, les
/// deux tests ci-dessus passeraient meme si plus rien n etait jamais
/// classe en ecriture.
#[test]
fn antivacuite_le_partage_se_fait_bien_sur_le_catalogue() {
    let (_td, db) = base();
    let jeton = CancelToken::new();
    jeton.cancel();

    let lecture = cancel::scope(&jeton, || db.session().execute("CALL obrain.pagerank()"));
    let ecriture = cancel::scope(&jeton, || db.session().execute("CALL copie()"));

    assert!(
        lecture.as_ref().err().is_some_and(|e| e.is_cancelled()),
        "le CALL de lecture doit etre annule : {lecture:?}"
    );
    assert!(
        !ecriture.as_ref().err().is_some_and(|e| e.is_cancelled()),
        "le CALL de la procedure UTILISATEUR ne doit pas etre annule : \
         {ecriture:?}"
    );
    assert_eq!(
        copies(&db),
        NOEUDS,
        "et son ecriture doit avoir eu lieu en entier"
    );
}

// ─────────────────────────────────────────────────────────────────────
//  III. LES SIX FAMILLES, ET L INVERSION DE POLARITE SUR LES NOMS
// ─────────────────────────────────────────────────────────────────────
//
// `plan_call_procedure` resout SIX familles. Une version precedente n en
// testait qu une — la procedure utilisateur — et rendait « lecture » pour
// les cinq autres. Or les familles 3 et 4 sont EXECUTEES pendant la
// planification, donc apres que la frontiere a arme le jeton, et au moins
// quatre d entre elles ecrivent. Mesure de la revue :
//
//     CALL obrain.cognitive.reinforce(1,2)
//       sans jeton -> Ok,  synapses 0 -> 1        (elle ECRIT)
//       jeton arme -> Err(Cancelled) ET synapses = 1
//
// Ecriture effectuee, requete rendue annulee, aucune transaction ouverte :
// rien a defaire. Ces tests classent, ce qui est exactement ce que la
// frontiere consomme (`Nature::Plan(has_mutations)`).

/// Classe un `CALL` designe par son NOM, sans passer par le parseur.
///
/// Certains segments (`create`, `drop`, ...) sont des mots reserves du
/// parseur GQL et ne peuvent pas s ecrire dans un `CALL` litteral — le
/// meme piege que `offset` rencontre plus tot dans ce chantier. Or ce que
/// la frontiere consomme est la CLASSIFICATION du plan, pas son texte : on
/// construit donc l operateur directement. `mute_texte` ci-dessous garde le
/// lien avec le traducteur pour les noms qui, eux, se parsent.
fn mute_nom(segments: &[&str]) -> bool {
    use obrain_engine::query::plan::{CallProcedureOp, LogicalOperator};
    let op = LogicalOperator::CallProcedure(CallProcedureOp {
        name: segments.iter().map(|s| (*s).to_string()).collect(),
        arguments: Vec::new(),
        yield_items: None,
    });
    let catalogue = obrain_engine::catalog::Catalog::new();
    op.mutations_avec(Some(&catalogue))
}

fn mute(requete: &str) -> bool {
    let plan = obrain_engine::query::translators::gql::translate(requete)
        .unwrap_or_else(|e| panic!("traduction de `{requete}` : {e}"));
    let catalogue = obrain_engine::catalog::Catalog::new();
    plan.root.mutations_avec(Some(&catalogue))
}

/// LE BLOQUANT. Quatre integrees qui ECRIVENT.
#[test]
fn les_integrees_qui_ecrivent_sont_classees_ecriture() {
    for nom in [
        ["obrain", "projection", "create"].as_slice(),
        ["obrain", "projection", "drop"].as_slice(),
        ["projection", "create"].as_slice(),
        ["projection", "drop"].as_slice(),
    ] {
        assert!(
            mute_nom(nom),
            "`{nom:?}` ECRIT et serait pourtant annulable : la frontiere \
             armerait le jeton sur elle, et aucune transaction ne serait \
             ouverte pour la defaire."
        );
    }
}

#[cfg(feature = "cognitive")]
#[test]
fn les_cognitives_qui_ecrivent_sont_classees_ecriture() {
    for nom in [
        ["obrain", "cognitive", "reinforce"].as_slice(),
        ["obrain", "engrams", "forget"].as_slice(),
        ["engrams", "forget"].as_slice(),
    ] {
        assert!(
            mute_nom(nom),
            "`{nom:?}` ECRIT et doit etre classee ecriture"
        );
    }
    for requete in [
        "CALL obrain.cognitive.reinforce(1, 2)",
        "CALL obrain.engrams.forget(1)",
    ] {
        assert!(
            mute(requete),
            "`{requete}` ECRIT — c est le chemin que le superviseur demande \
             explicitement a l entite d emprunter, via `obrain.query` qui \
             passe par `execute_annulable`. Un effacement RGPD a moitie fait \
             ne se rattrape pas."
        );
    }
}

/// L INVERSION ELLE-MEME : ce qui n est pas declare lecture est ecriture.
/// C est ce test qui distingue « j ai corrige les quatre » de « la
/// cinquieme ne peut pas passer ».
#[test]
fn une_integree_inconnue_est_classee_ecriture() {
    assert!(
        mute_nom(["obrain", "famille", "ajoutee_demain"].as_slice()),
        "une integree inconnue doit compter comme une ecriture"
    );
    assert!(
        mute("CALL obrain.famille.ajoutee_demain(1)"),
        "une procedure que personne n a declaree LECTURE doit compter comme \
         une ecriture. Sans cela, chaque integree ajoutee demain rouvre le \
         meme trou."
    );
}

/// TEMOINS DE LECTURE. Sans eux, tout ce qui precede passerait avec un
/// `procedure_mute` qui rendrait `true` en bloc — la regression `pagerank`.
#[test]
fn temoins_les_integrees_qui_lisent_restent_des_lectures() {
    for nom in [
        ["obrain", "pagerank"].as_slice(),
        ["pagerank"].as_slice(),
        ["obrain", "procedures"].as_slice(),
        ["db", "labels"].as_slice(),
        ["db", "relationshipTypes"].as_slice(),
        ["db", "propertyKeys"].as_slice(),
        ["obrain", "projection", "list"].as_slice(),
        ["obrain", "sssp"].as_slice(),
    ] {
        assert!(
            !mute_nom(nom),
            "`{nom:?}` LIT : la classer en ecriture lui retirerait \
             l annulation (regression `pagerank`)"
        );
    }
    for requete in [
        "CALL obrain.pagerank()",
        "CALL obrain.procedures()",
        "CALL db.labels()",
        "CALL obrain.projection.list()",
    ] {
        assert!(
            !mute(requete),
            "`{requete}` LIT : la classer en ecriture lui retirerait \
             l annulation, ce qui est la regression que ce lot a deja \
             corrigee une fois."
        );
    }
}

/// ANTI-VACUITE DE LA SECTION : la classification distingue toujours une
/// lecture ordinaire d une ecriture ordinaire.
#[test]
fn antivacuite_la_classification_distingue_encore_lecture_et_ecriture() {
    assert!(
        !mute("MATCH (n) RETURN count(n)"),
        "une lecture reste lecture"
    );
    assert!(mute("CREATE (n:X {v: 1})"), "une ecriture reste ecriture");
}
