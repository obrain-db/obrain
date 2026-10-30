//! ── UNE FONCTION FOURNIE PAR L'HÔTE EST APPELABLE DANS UNE REQUÊTE ─────────
//!
//! Défaut mesuré le 2026-09-16 : `PluginRegistry` (dans `obrain-adapters`)
//! portait `register_udf`, était thread-safe, sa documentation annonçait
//! « résolu au moment du planning » — et **aucun appelant de production ne le
//! lisait**. On pouvait enregistrer une fonction sans qu'aucune requête puisse
//! l'appeler : `RETURN mon_algo(n)` répondait « fonction inconnue », avec un
//! message qui invitait même à vérifier l'orthographe.
//!
//! Le registre est maintenant branché sur l'évaluateur par un point d'extension
//! du noyau (`obrain_core::execution::fonction_hote`), installé à la
//! construction de chaque base. Ce fichier éprouve la chaîne ENTIÈRE :
//! enregistrement → résolution → évaluation, avec le comportement d'erreur.
//!
//! Il éprouve aussi ce qui n'a PAS changé : une fonction vraiment inconnue doit
//! continuer de lever (ISO/IEC 39075 §23) — brancher un hôte ne doit pas
//! transformer une faute de frappe en colonne vide.

use std::sync::Arc;

use obrain_adapters::plugins::{PluginRegistry, UserDefinedFunction, registre_global};
use obrain_common::types::{Value as TValue, NodeId};
use obrain_engine::ObrainDB;

/// Une fonction de test : `double(x)` rend `2 * x` pour un entier.
struct Double;

impl UserDefinedFunction for Double {
    fn name(&self) -> &'static str {
        "double"
    }
    fn description(&self) -> &'static str {
        "rend le double de son argument entier"
    }
    fn evaluate(&self, args: &[TValue]) -> obrain_common::utils::error::Result<TValue> {
        match args.first() {
            Some(TValue::Int64(n)) => Ok(TValue::Int64(n * 2)),
            autre => Err(obrain_common::utils::error::Error::Internal(format!(
                "double attend un entier, reçu {autre:?}"
            ))),
        }
    }
}

/// Une fonction qui ÉCHOUE toujours : on veut voir le message remonter tel quel.
struct Rate;

impl UserDefinedFunction for Rate {
    fn name(&self) -> &'static str {
        "rate_toujours"
    }
    fn description(&self) -> &'static str {
        "échoue, pour éprouver la remontée du message"
    }
    fn evaluate(&self, _args: &[TValue]) -> obrain_common::utils::error::Result<TValue> {
        Err(obrain_common::utils::error::Error::Internal(
            "raison lisible par l'utilisateur".to_string(),
        ))
    }
}

#[test]
fn une_udf_enregistree_est_appelable_dans_une_requete() {
    // L'enregistrement se fait par le registre DU PROCESSUS — celui que le
    // moteur consulte depuis qu'il est branché.
    registre_global().register_udf(Arc::new(Double));
    assert!(
        registre_global().list_udfs().iter().any(|n| n == "double"),
        "l'UDF doit être listée : c'est la surface qui la rend atteignable"
    );

    let db = ObrainDB::new_in_memory();
    db.create_node(&["L"]);
    let valeur = db
        .execute("MATCH (n:L) RETURN double(21) AS d")
        .expect("une UDF enregistrée ne doit plus être « fonction inconnue »")
        .rows
        .first()
        .and_then(|r| r.first().cloned());
    assert_eq!(valeur, Some(TValue::Int64(42)), "double(21) doit rendre 42");
}

#[test]
fn une_udf_qui_echoue_dit_pourquoi() {
    registre_global().register_udf(Arc::new(Rate));
    let db = ObrainDB::new_in_memory();
    db.create_node(&["L"]);
    let err = db
        .execute("MATCH (n:L) RETURN rate_toujours(1) AS r")
        .expect_err("une UDF qui échoue doit faire échouer la requête");
    let message = format!("{err:?}");
    assert!(
        message.contains("raison lisible par l'utilisateur"),
        "le message de l'hôte doit remonter tel quel : {message}"
    );
}

#[test]
fn une_fonction_vraiment_inconnue_leve_toujours() {
    // Le contrat GQL (ISO/IEC 39075 §23, condition 42 000) ne doit pas avoir été
    // dilué par le point d'extension : sans hôte pour ce nom, l'erreur reste.
    let db = ObrainDB::new_in_memory();
    db.create_node(&["L"]);
    let err = db
        .execute("MATCH (n:L) RETURN fonction_qui_n_existe_pas(1) AS x")
        .expect_err("une fonction inconnue doit toujours lever");
    assert!(
        format!("{err:?}").contains("fonction inconnue"),
        "le message d'origine doit être conservé : {err:?}"
    );
}

#[test]
fn le_registre_est_unique_par_processus() {
    // Deux appels rendent le MÊME registre : c'est ce qui rend l'enregistrement
    // d'une UDF visible depuis n'importe quelle base du processus.
    registre_global().register_udf(Arc::new(Double));
    let avant = registre_global().list_udfs().len();
    let apres = PluginRegistry::new().list_udfs().len();
    assert!(avant >= 1, "le registre du processus porte au moins l'UDF de test");
    assert_eq!(apres, 0, "un registre NEUF est vide — le global est bien partagé");
}

#[test]
fn l_injection_de_l_hote_ne_touche_pas_les_donnees() {
    // Le point d'extension ne doit rien changer d'autre : une requête ordinaire
    // rend les mêmes lignes qu'avant.
    let db = ObrainDB::new_in_memory();
    for i in 0..3 {
        db.create_node(&["L"]);
        let _ = i;
    }
    let compte = db
        .execute("MATCH (n:L) RETURN count(n) AS n")
        .expect("requête ordinaire")
        .rows
        .first()
        .and_then(|r| r.first().cloned());
    assert_eq!(compte, Some(TValue::Int64(3)));
    let _ = NodeId(1);
}
